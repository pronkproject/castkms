// SPDX-License-Identifier: GPL-2.0-only

//! One development output, without a presentation clock or pixel consumer.

use super::Driver;
use core::marker::PhantomData;
use crtc::RawCrtc;
use kernel::{
    device,
    drm::{
        fourcc,
        kms::*,
        Device, //
    },
    prelude::*, //
};
use plane::RawPlaneState;

#[pin_data]
pub(super) struct Plane {}
#[pin_data]
pub(super) struct Crtc {}
#[pin_data]
pub(super) struct Encoder {}
#[pin_data]
pub(super) struct Connector {}

pub(super) struct State;

impl plane::DriverPlaneState for State {
    type Plane = Plane;
    fn new(_: &plane::Plane<Plane>) -> Result<Self> {
        Ok(Self)
    }
    fn duplicate(&self) -> Result<Self> {
        Ok(Self)
    }
}

impl crtc::DriverCrtcState for State {
    type Crtc = Crtc;
    fn new(_: &crtc::Crtc<Crtc>) -> Result<Self> {
        Ok(Self)
    }
    fn duplicate(&self) -> Result<Self> {
        Ok(Self)
    }
}

impl connector::DriverConnectorState for State {
    type Connector = Connector;
    fn new(_: &connector::Connector<Connector>) -> Result<Self> {
        Ok(Self)
    }
    fn duplicate(&self) -> Result<Self> {
        Ok(Self)
    }
}

#[vtable]
impl plane::DriverPlane for Plane {
    type Args = ();
    type Driver = Driver;
    type State = State;

    fn new(_: &Device<Driver>, _: ()) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {})
    }

    fn atomic_check(check: plane::PlaneAtomicCheck<'_, Self>) -> Result {
        let (transaction, mut state) = check.take_state_new_state();
        if let Some(crtc) = state.crtc() {
            let crtc_state = transaction.add_crtc_state(crtc)?;
            state.atomic_helper_check(&crtc_state, false, false)?;
        }
        Ok(())
    }
}

#[vtable]
impl crtc::DriverCrtc for Crtc {
    type Args = ();
    type Driver = Driver;
    type State = State;
    type VblankImpl = PhantomData<Self>;

    fn new(_: &Device<Driver>, _: &()) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {})
    }
}

#[vtable]
impl encoder::DriverEncoder for Encoder {
    type Args = ();
    type Driver = Driver;

    fn new(_: &Device<Driver>, _: ()) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {})
    }
}

#[vtable]
impl connector::DriverConnector for Connector {
    type Args = ();
    type Driver = Driver;
    type State = State;

    fn new(_: &Device<Driver>, _: ()) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {})
    }

    fn get_modes<'a>(
        connector: connector::ConnectorGuard<'a, Self>,
        _: &ModeConfigGuard<'a, Driver>,
    ) -> i32 {
        let count = connector.add_modes_noedid((1920, 1080));
        connector.set_preferred_mode((1920, 1080));
        count
    }
}

#[vtable]
impl KmsDriver for Driver {
    type Connector = Connector;
    type Plane = Plane;
    type Crtc = Crtc;
    type Encoder = Encoder;

    fn mode_config_info(_: &device::Device, _: &()) -> Result<ModeConfigInfo> {
        Ok(ModeConfigInfo {
            min_resolution: (1, 1),
            max_resolution: (1920, 1080),
            max_cursor: (0, 0),
            preferred_depth: 24,
            preferred_fourcc: Some(fourcc::XRGB8888),
            enable_default_client: false,
        })
    }

    fn create_objects(dev: &UnregisteredKmsDevice<'_, Self>) -> Result {
        let plane = plane::UnregisteredPlane::<Plane>::new(
            dev,
            0,
            &[fourcc::XRGB8888],
            Some(&[fourcc::FORMAT_MOD_LINEAR]),
            plane::Type::Primary,
            None,
            (),
        )?;
        let crtc = crtc::UnregisteredCrtc::<Crtc>::new(
            dev,
            plane,
            None::<&plane::UnregisteredPlane<Plane>>,
            None,
            (),
        )?;
        let encoder = encoder::UnregisteredEncoder::<Encoder>::new(
            dev,
            encoder::Type::Virtual,
            crtc.mask(),
            0,
            None,
            (),
        )?;
        let connector =
            connector::UnregisteredConnector::<Connector>::new(dev, connector::Type::Virtual, ())?;
        connector.attach_encoder(encoder)
    }

    fn atomic_commit_tail<'a>(
        mut tail: atomic::AtomicCommitTail<'a, Self>,
        modesets: atomic::ModesetsReadyToken<'a, Self>,
        planes: atomic::PlaneUpdatesReadyToken<'a, Self>,
    ) -> atomic::CommittedAtomicState<'a, Self> {
        let disabled = tail.commit_modeset_disables(modesets);
        let planes = tail.commit_planes(planes, atomic::PlaneCommitFlags::default());
        let enabled = tail.commit_modeset_enables(disabled);
        // No pixel reader survives the commit. Complete events immediately using DRM's
        // no-vblank path, not a claim that a receiver presented the frame.
        tail.fake_vblank();
        tail.commit_hw_done(enabled, planes)
    }
}
