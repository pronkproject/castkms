// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Event consumers with native vblank storage, separate from the no-vblank setup tests.

use super::{framebuffer, mode, Counts, Lifetime};
use crate::{
    device,
    drm::{self, fourcc, gem, kms::*, Device, UnregisteredDevice},
    faux,
    interrupt::LocalInterruptDisabled,
    prelude::*,
    sync::{aref::ARef, Arc, Completion},
    workqueue,
};
use core::sync::atomic::{AtomicI32, AtomicPtr, AtomicU32, Ordering};
use crtc::{AsRawCrtc, RawCrtc, RawCrtcState};

#[pin_data]
struct Observations {
    counts: Arc<Counts>,
    delay: AtomicU32,
    fail_enable: AtomicU32,
    enable_calls: AtomicU32,
    event_error: AtomicI32,
    armed: AtomicU32,
    clock: AtomicI32,
    #[pin]
    programmed: Completion,
}

#[pin_data]
struct Data {
    observations: Arc<Observations>,
    // Initialized before setup returns; consumers retain the device and exclude teardown.
    crtc: AtomicPtr<bindings::drm_crtc>,
    connector: AtomicPtr<bindings::drm_connector>,
}

struct EventDriver;
struct EventFile;
#[pin_data]
struct EventObject {}
#[pin_data]
struct EventPlane {
    _life: Lifetime,
}
#[pin_data]
struct EventCrtc {
    _life: Lifetime,
}
#[pin_data]
struct EventEncoder {
    _life: Lifetime,
}
#[pin_data]
struct EventConnector {
    _life: Lifetime,
}
struct State;

impl plane::DriverPlaneState for State {
    type Plane = EventPlane;
    fn new(_: &plane::Plane<EventPlane>) -> Result<Self> {
        Ok(Self)
    }
    fn duplicate(&self) -> Result<Self> {
        Ok(Self)
    }
}

impl crtc::DriverCrtcState for State {
    type Crtc = EventCrtc;
    fn new(_: &crtc::Crtc<EventCrtc>) -> Result<Self> {
        Ok(Self)
    }
    fn duplicate(&self) -> Result<Self> {
        Ok(Self)
    }
}

impl connector::DriverConnectorState for State {
    type Connector = EventConnector;
    fn new(_: &connector::Connector<EventConnector>) -> Result<Self> {
        Ok(Self)
    }
    fn duplicate(&self) -> Result<Self> {
        Ok(Self)
    }
}

impl drm::file::DriverFile for EventFile {
    type Driver = EventDriver;
    fn open(_: &Device<EventDriver>) -> Result<Pin<KBox<Self>>> {
        Ok(KBox::new(Self, GFP_KERNEL)?.into())
    }
}

impl gem::DriverObject for EventObject {
    type Driver = EventDriver;
    type Args = ();
    fn new(_: &Device<EventDriver>, _: usize, _: ()) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {})
    }
}

#[vtable]
impl drm::Driver for EventDriver {
    type Data = Data;
    type RegistrationData<'a> = ();
    type File = EventFile;
    type Object = gem::shmem::Object<EventObject>;
    type ParentDevice<C: device::DeviceContext> = faux::Device<C>;
    type Kms = Self;
    const INFO: drm::DriverInfo = drm::DriverInfo {
        major: 0,
        minor: 0,
        patchlevel: 0,
        name: c"rust_kms_events",
        desc: c"Rust KMS event tests",
    };
    const IOCTLS: &'static [drm::ioctl::DrmIoctlDescriptor] = &[];
}

#[vtable]
impl plane::DriverPlane for EventPlane {
    type Args = ();
    type Driver = EventDriver;
    type State = State;
    fn new(dev: &Device<EventDriver>, _: ()) -> impl PinInit<Self, Error> {
        Ok(Self {
            _life: Lifetime::new(&dev.observations.counts),
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
}

#[vtable]
impl crtc::DriverCrtc for EventCrtc {
    type Args = ();
    type Driver = EventDriver;
    type State = State;
    type VblankImpl = Self;
    fn new(dev: &Device<EventDriver>, _: &()) -> impl PinInit<Self, Error> {
        Ok(Self {
            _life: Lifetime::new(&dev.observations.counts),
        })
    }
    fn atomic_enable(mut commit: crtc::CrtcAtomicCommit<'_, Self>) {
        commit.crtc().vblank_on();
        if let Some(event) = commit.get_pending_vblank_event() {
            event.send();
        }
    }
    fn atomic_disable(mut commit: crtc::CrtcAtomicCommit<'_, Self>) {
        commit.crtc().vblank_off();
        if let Some(event) = commit.get_pending_vblank_event() {
            event.send();
        }
    }
    fn atomic_flush(mut commit: crtc::CrtcAtomicCommit<'_, Self>) {
        let (old, new) = commit.old_new_state();
        if !old.active() || !new.active() || new.mode_changed() {
            return;
        }
        let crtc = commit.crtc();
        let observations = &crtc.drm_dev().observations;
        observations
            .clock
            .store(new.adjusted_mode().crtc_clock(), Ordering::Relaxed);
        if observations.delay.load(Ordering::Relaxed) != 0 {
            let result = crtc.vblank_get().and_then(|reference| {
                commit
                    .get_pending_vblank_event()
                    .ok_or(ENOENT)?
                    .arm(reference)
            });
            match result {
                Ok(()) => {
                    observations.armed.store(1, Ordering::Relaxed);
                }
                Err(error) => {
                    observations
                        .event_error
                        .store(error.to_errno(), Ordering::Relaxed);
                }
            }
        }
        // Immediate updates and rejected arms must still finish their native event.
        if let Some(event) = commit.get_pending_vblank_event() {
            event.send();
        }
    }
}

impl vblank::VblankSupport for EventCrtc {
    type Crtc = Self;
    fn enable_vblank(
        crtc: &crtc::Crtc<Self>,
        _: &vblank::VblankGuard<'_, Self>,
        _: &LocalInterruptDisabled,
    ) -> Result {
        let observations = &crtc.drm_dev().observations;
        observations.enable_calls.fetch_add(1, Ordering::Relaxed);
        if observations.fail_enable.load(Ordering::Relaxed) != 0 {
            return Err(EIO);
        }
        Ok(())
    }
    fn disable_vblank(
        _: &crtc::Crtc<Self>,
        _: &vblank::VblankGuard<'_, Self>,
        _: &LocalInterruptDisabled,
    ) {
    }
    fn get_vblank_timestamp(_: &crtc::Crtc<Self>, _: bool) -> Option<vblank::VblankTimestamp> {
        None
    }
}

#[vtable]
impl encoder::DriverEncoder for EventEncoder {
    type Args = ();
    type Driver = EventDriver;
    fn new(dev: &Device<EventDriver>, _: ()) -> impl PinInit<Self, Error> {
        Ok(Self {
            _life: Lifetime::new(&dev.observations.counts),
        })
    }
}

#[vtable]
impl connector::DriverConnector for EventConnector {
    type Args = ();
    type Driver = EventDriver;
    type State = State;
    fn new(dev: &Device<EventDriver>, _: ()) -> impl PinInit<Self, Error> {
        Ok(Self {
            _life: Lifetime::new(&dev.observations.counts),
        })
    }
    fn get_modes<'a>(
        connector: connector::ConnectorGuard<'a, Self>,
        _: &ModeConfigGuard<'a, EventDriver>,
    ) -> i32 {
        connector.add_modes_noedid((640, 480))
    }
}

#[vtable]
impl KmsDriver for EventDriver {
    type Connector = EventConnector;
    type Plane = EventPlane;
    type Crtc = EventCrtc;
    type Encoder = EventEncoder;
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
        let plane = plane::UnregisteredPlane::<EventPlane>::new(
            dev,
            0,
            &[fourcc::XRGB8888],
            Some(&[fourcc::FORMAT_MOD_LINEAR]),
            plane::Type::Primary,
            None,
            (),
        )?;
        let crtc = crtc::UnregisteredCrtc::<EventCrtc>::new(
            dev,
            plane,
            None::<&plane::UnregisteredPlane<EventPlane>>,
            None,
            (),
        )?;
        let encoder = encoder::UnregisteredEncoder::<EventEncoder>::new(
            dev,
            encoder::Type::Virtual,
            crtc.mask(),
            0,
            None,
            (),
        )?;
        let connector = connector::UnregisteredConnector::<EventConnector>::new(
            dev,
            connector::Type::Virtual,
            (),
        )?;
        connector.attach_encoder(encoder)?;
        dev.crtc.store(crtc.as_raw(), Ordering::Relaxed);
        dev.connector.store(connector.as_raw(), Ordering::Relaxed);
        Ok(())
    }
    fn atomic_commit_tail<'a>(
        mut tail: atomic::AtomicCommitTail<'a, Self>,
        modesets: atomic::ModesetsReadyToken<'a, Self>,
        planes: atomic::PlaneUpdatesReadyToken<'a, Self>,
    ) -> atomic::CommittedAtomicState<'a, Self> {
        let observations = tail.drm_dev().observations.clone();
        let disabled = tail.commit_modeset_disables(modesets);
        let planes = tail.commit_planes(planes, atomic::PlaneCommitFlags::default());
        let enabled = tail.commit_modeset_enables(disabled);
        let committed = tail.commit_hw_done(enabled, planes);
        if observations.delay.load(Ordering::Relaxed) != 0 {
            observations.programmed.complete_all();
        }
        // Deliberately omit the explicit flip wait: the completed-state owner must perform it.
        committed
    }
}

fn framebuffer_count(dev: &Device<EventDriver>) -> i32 {
    // SAFETY: Setup initialized the mutex. The device borrow excludes destruction, and this
    // lock serializes count inspection with any unexpected early framebuffer cleanup.
    unsafe {
        let config = &raw mut (*dev.as_raw()).mode_config;
        bindings::mutex_lock(&raw mut (*config).fb_lock);
        let count = (*config).num_fb;
        bindings::mutex_unlock(&raw mut (*config).fb_lock);
        count
    }
}

fn vblank_references(crtc: &crtc::Crtc<EventCrtc>) -> i32 {
    use crate::sync::atomic::{Atomic, Relaxed};

    // SAFETY: The initialized CRTC borrows its device's vblank allocation. The native
    // refcount is an aligned atomic_t; use an LKMM atomic load of its counter field.
    unsafe { Atomic::<i32>::from_ptr(&raw mut (*crtc.vblank_crtc().as_raw()).refcount.counter) }
        .load(Relaxed)
}

fn create(
    parent: &faux::Device<device::Bound>,
    counts: &Arc<Counts>,
) -> Result<UnregisteredDevice<EventDriver>> {
    let observations = Arc::pin_init(
        pin_init!(Observations {
            counts: counts.clone(), delay: AtomicU32::new(0),
            fail_enable: AtomicU32::new(0), enable_calls: AtomicU32::new(0),
            event_error: AtomicI32::new(0), armed: AtomicU32::new(0),
            clock: AtomicI32::new(0), programmed <- Completion::new(),
        }),
        GFP_KERNEL,
    )?;
    let drm = UnregisteredDevice::<EventDriver>::new(
        parent,
        try_pin_init!(Data {
            observations,
            crtc: AtomicPtr::new(core::ptr::null_mut()),
            connector: AtomicPtr::new(core::ptr::null_mut()),
        }),
    )?;
    // SAFETY: Newly allocated device, exclusively owned until setup finishes.
    unsafe { <EventDriver as private::KmsImpl>::setup_kms(&drm) }?;
    Ok(drm)
}

struct FlipResult {
    pending: bool,
    delivered: bool,
    before: i32,
    after: i32,
    disabled: i32,
    error: i32,
    observations: Arc<Observations>,
}

fn delayed_flip(deliver: impl FnOnce(&crtc::Crtc<EventCrtc>) -> bool) -> Result<FlipResult> {
    let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
    let parent = faux::Registration::new(c"rust-kms-event", None)?;
    let drm = create(parent.as_ref(), &counts)?;
    let observations = drm.observations.clone();
    let first = framebuffer(&drm)?;
    let second = framebuffer(&drm)?;
    let mode = mode()?;
    // Allocate the worker handshakes before enabling scanout, so failure cannot leave vblank on.
    let done = Arc::pin_init(Completion::new(), GFP_KERNEL)?;
    let finished = Arc::new(AtomicU32::new(0), GFP_KERNEL)?;
    let result = Arc::new(AtomicI32::new(0), GFP_KERNEL)?;
    // SAFETY: Setup has finished and the device owns its objects until test teardown.
    let crtc = unsafe { crtc::Crtc::<EventCrtc>::from_raw(drm.crtc.load(Ordering::Relaxed)) };
    let connector = unsafe {
        <connector::Connector<EventConnector> as connector::AsRawConnector>::from_raw(
            drm.connector.load(Ordering::Relaxed),
        )
    };
    let scanout = atomic::CrtcScanout {
        mode: &mode,
        framebuffer: &first,
        connectors: &[connector],
        position: (0, 0),
    };
    // SAFETY: Initialized device, with registration, object creation and teardown excluded.
    unsafe { atomic::run_update(&drm, |state| state.set_crtc_config(crtc, Some(&scanout))) }?;
    drop(first);
    let worker_done = done.clone();
    let worker_finished = finished.clone();
    let worker_result = result.clone();
    let worker_dev: ARef<Device<EventDriver>> = (&*drm).into();
    observations.delay.store(1, Ordering::Relaxed);
    let spawned = workqueue::system_dfl().try_spawn(GFP_KERNEL, move || {
        // SAFETY: The main task excludes setup, registration and teardown until join.
        let result = unsafe {
            let crtc = crtc::Crtc::<EventCrtc>::from_raw(worker_dev.crtc.load(Ordering::Relaxed));
            let connector =
                <connector::Connector<EventConnector> as connector::AsRawConnector>::from_raw(
                    worker_dev.connector.load(Ordering::Relaxed),
                );
            let scanout = atomic::CrtcScanout {
                mode: &mode,
                framebuffer: &second,
                connectors: &[connector],
                position: (0, 0),
            };
            atomic::run_update(&worker_dev, |state| {
                state.set_crtc_config(crtc, Some(&scanout))
            })
        };
        worker_result.store(result.err().map_or(0, Error::to_errno), Ordering::Release);
        // Wake the main task even when an error prevents entering the commit tail.
        worker_dev.observations.programmed.complete_all();
        drop(second);
        drop(worker_dev);
        worker_finished.store(1, Ordering::Release);
        worker_done.complete_all();
    });
    if let Err(error) = spawned {
        observations.delay.store(0, Ordering::Relaxed);
        // SAFETY: Spawn failed; the test still exclusively owns the initialized device.
        unsafe { atomic::run_update(&drm, |state| state.set_crtc_config(crtc, None)) }?;
        return Err(error.into());
    }
    observations.programmed.wait_for_completion();
    // Give the worker a scheduling opportunity after hardware completion. A missing implicit
    // flip wait must not pass merely because the main task sampled before the tail returned.
    // SAFETY: This task holds no spinlock and may sleep. No display resources are released.
    unsafe { bindings::msleep(20) };
    let pending = finished.load(Ordering::Acquire) == 0;
    let before = framebuffer_count(&drm);
    let delivered = deliver(crtc);
    done.wait_for_completion();
    let after = framebuffer_count(&drm);
    observations.delay.store(0, Ordering::Relaxed);
    // SAFETY: The worker has returned and dropped its device and framebuffer references.
    unsafe { atomic::run_update(&drm, |state| state.set_crtc_config(crtc, None)) }?;
    let disabled = framebuffer_count(&drm);
    drop(drm);
    drop(parent);
    Ok(FlipResult {
        pending,
        delivered,
        before,
        after,
        disabled,
        error: result.load(Ordering::Acquire),
        observations,
    })
}

#[kunit_tests(rust_drm_events)]
mod cases {
    use super::*;

    #[test]
    fn owned_vblank_reference_retains_device() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-owned-vblank", None)?;
        let drm = create(parent.as_ref(), &counts)?;
        // SAFETY: Setup completed and the device owns the CRTC until its final reference drops.
        let crtc = unsafe { crtc::Crtc::<EventCrtc>::from_raw(drm.crtc.load(Ordering::Relaxed)) };
        crtc.vblank_on();
        let owned = match crtc.vblank_get() {
            Ok(reference) => reference.into_owned(),
            Err(error) => {
                crtc.vblank_off();
                return Err(error);
            }
        };
        let transferred = vblank_references(owned.crtc());
        drop(drm);
        let retained = counts.objects.load(Ordering::Relaxed);
        // Disable before teardown. Native off contributes its own reference, independently
        // of the reference being tested. A separate owner permits inspection after its drop.
        owned.crtc().vblank_off();
        let off = vblank_references(owned.crtc());
        let observer = owned.crtc().to_owned_ref();
        drop(owned);
        let released = vblank_references(observer.crtc());
        drop(observer);
        drop(parent);
        assert_eq!(transferred, 1);
        assert_eq!(retained, 4);
        assert_eq!(off, 2);
        assert_eq!(released, 1);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn rejected_vblank_enable_balances_reference() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-vblank-enable-error", None)?;
        let drm = create(parent.as_ref(), &counts)?;
        let observations = drm.observations.clone();
        // SAFETY: Setup completed and teardown is excluded until the test finishes.
        let crtc = unsafe { crtc::Crtc::<EventCrtc>::from_raw(drm.crtc.load(Ordering::Relaxed)) };
        crtc.vblank_on();
        observations.fail_enable.store(1, Ordering::Relaxed);
        let rejected = crtc.vblank_get().err().map(Error::to_errno);
        let after_rejection = vblank_references(crtc);
        observations.fail_enable.store(0, Ordering::Relaxed);
        let retried = crtc.vblank_get().map(|reference| {
            let held = vblank_references(crtc);
            drop(reference);
            (held, vblank_references(crtc))
        });
        crtc.vblank_off();
        drop(drm);
        drop(parent);
        assert_eq!(rejected, Some(EIO.to_errno()));
        assert_eq!(after_rejection, 0);
        assert_eq!(retried?, (1, 0));
        assert_eq!(observations.enable_calls.load(Ordering::Relaxed), 2);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn vblank_off_rejects_new_references() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-vblank-off-get", None)?;
        let drm = create(parent.as_ref(), &counts)?;
        // SAFETY: Setup completed and teardown is excluded until the test finishes.
        let crtc = unsafe { crtc::Crtc::<EventCrtc>::from_raw(drm.crtc.load(Ordering::Relaxed)) };
        crtc.vblank_on();
        let result = (|| -> Result<_> {
            let held = crtc.vblank_get()?;
            crtc.vblank_off();
            let rejected = crtc.vblank_get().err().map(Error::to_errno);
            let after_rejection = vblank_references(crtc);
            drop(held);
            let after_drop = vblank_references(crtc);
            crtc.vblank_on();
            let retried = crtc.vblank_get()?;
            let after_retry = vblank_references(crtc);
            drop(retried);
            Ok((rejected, after_rejection, after_drop, after_retry))
        })();
        crtc.vblank_off();
        drop(drm);
        drop(parent);
        assert_eq!(result?, (Some(EINVAL.to_errno()), 2, 1, 1));
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn delayed_flip_retains_old_framebuffer() -> Result {
        let result = delayed_flip(|crtc| crtc.handle_vblank())?;
        // Assert only after releasing the pending worker and all display resources.
        assert!(result.pending);
        assert!(result.delivered);
        assert_eq!(result.before, 2);
        assert_eq!(result.after, 1);
        assert_eq!(result.disabled, 0);
        assert_eq!(result.error, 0);
        assert_eq!(result.observations.armed.load(Ordering::Relaxed), 1);
        assert_eq!(result.observations.event_error.load(Ordering::Relaxed), 0);
        assert_eq!(result.observations.clock.load(Ordering::Relaxed), 25175);
        assert_eq!(
            result.observations.counts.objects.load(Ordering::Relaxed),
            0
        );
        Ok(())
    }

    #[test]
    fn vblank_off_drains_armed_flip() -> Result {
        let result = delayed_flip(|crtc| {
            crtc.vblank_off();
            !crtc.handle_vblank()
        })?;
        assert!(result.pending);
        assert!(result.delivered);
        assert_eq!(result.before, 2);
        assert_eq!(result.after, 1);
        assert_eq!(result.disabled, 0);
        assert_eq!(result.error, 0);
        assert_eq!(result.observations.armed.load(Ordering::Relaxed), 1);
        assert_eq!(result.observations.event_error.load(Ordering::Relaxed), 0);
        assert_eq!(
            result.observations.counts.objects.load(Ordering::Relaxed),
            0
        );
        Ok(())
    }
}
