// SPDX-License-Identifier: GPL-2.0-only

//! One development output and its accepted display descriptions.

use super::{
    monitor,
    output::SceneUpdate,
    provenance::{
        PreviousOwner,
        Provenance,
        Selection, //
    },
    scene,
    Driver, //
};
use crtc::{
    RawCrtc,
    RawCrtcState, //
};
use kernel::{
    device,
    drm::{
        fourcc,
        kms::*,
        Device, //
    },
    prelude::*,
    sync::Arc, //
};
use plane::RawPlaneState;

#[pin_data]
pub(super) struct Plane {}
#[pin_data]
pub(super) struct Crtc {}
#[pin_data]
pub(super) struct Encoder {}
#[pin_data]
pub(super) struct Connector {
    monitor: Arc<monitor::Monitor>,
}

pub(super) struct ConnectorState;

pub(super) struct CrtcState {
    configuration: Option<scene::Configuration>,
    visible: bool,
    blank_owner: Option<kernel::drm::auth::MasterRef<Driver>>,
}

pub(super) struct PlaneState {
    geometry: Option<scene::Geometry>,
    content: Option<scene::ContentSerial>,
    selection: Selection,
    owner: Option<kernel::drm::auth::MasterRef<Driver>>,
    producer: Option<Arc<framebuffer::dependencies::Dependencies>>,
}

impl plane::DriverPlaneState for PlaneState {
    type Plane = Plane;
    fn new(_: &plane::Plane<Plane>) -> Result<Self> {
        Ok(Self {
            geometry: None,
            content: None,
            selection: Selection::RetainedFramebuffer,
            owner: None,
            producer: None,
        })
    }
    fn duplicate(&self) -> Result<Self> {
        Ok(Self {
            geometry: None,
            content: self.content,
            selection: Selection::RetainedFramebuffer,
            owner: self.owner.clone(),
            producer: None,
        })
    }
}

impl crtc::DriverCrtcState for CrtcState {
    type Crtc = Crtc;
    fn new(_: &crtc::Crtc<Crtc>) -> Result<Self> {
        Ok(Self {
            configuration: None,
            visible: false,
            blank_owner: None,
        })
    }
    fn duplicate(&self) -> Result<Self> {
        Ok(Self {
            configuration: self.configuration.clone(),
            visible: self.visible,
            blank_owner: self.blank_owner.clone(),
        })
    }
}

impl connector::DriverConnectorState for ConnectorState {
    type Connector = Connector;
    fn new(_: &connector::Connector<Connector>) -> Result<Self> {
        Ok(Self)
    }
    fn duplicate(&self) -> Result<Self> {
        Ok(Self)
    }
}

fn check_geometry(
    transaction: &atomic::AtomicStateComposer<Driver>,
    state: &mut plane::PlaneStateMutator<'_, plane::PlaneState<PlaneState>>,
) -> Result {
    state.geometry = None;
    if let Some(crtc) = state.crtc() {
        let crtc_state = transaction.add_crtc_state(crtc)?;
        state.atomic_helper_check(&crtc_state, false, false)?;
        if crtc_state.active() && state.visible() {
            state.geometry = Some(scene::Geometry {
                source: [
                    state.source_x_16_16(),
                    state.source_y_16_16(),
                    state.source_width_16_16(),
                    state.source_height_16_16(),
                ],
                destination: [state.crtc_w(), state.crtc_h()],
                output: [
                    u32::from(crtc_state.mode().hdisplay()),
                    u32::from(crtc_state.mode().vdisplay()),
                ],
            });
        }
    }
    Ok(())
}

fn resolve_owner(
    old: &plane::PlaneState<PlaneState>,
    state: &plane::PlaneStateMutator<'_, plane::PlaneState<PlaneState>>,
    current: Option<&kernel::drm::auth::MasterRef<Driver>>,
) -> Option<kernel::drm::auth::MasterRef<Driver>> {
    let framebuffer = state.framebuffer()?;
    let previous = if old
        .framebuffer()
        .is_some_and(|old| core::ptr::eq(old, framebuffer))
    {
        PreviousOwner::SameFramebuffer(old.owner.as_ref())
    } else {
        PreviousOwner::DifferentFramebuffer
    };
    Provenance::for_update(framebuffer.data(), previous, current, state.selection).cloned()
}

#[vtable]
impl plane::DriverPlane for Plane {
    type Args = ();
    type Driver = Driver;
    type State = PlaneState;

    fn new(_: &Device<Driver>, _: ()) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {})
    }

    fn atomic_check(check: plane::PlaneAtomicCheck<'_, Self>) -> Result {
        let (transaction, old, mut state) = check.take_all();
        check_geometry(transaction, &mut state)?;
        if let Some(geometry) = state.geometry {
            super::execution::host::check_framebuffer(
                state.framebuffer().ok_or(EINVAL)?,
                geometry,
            )?;
        }
        state.content = scene::ContentSerial::for_update(old.content, state.geometry.is_some())?;
        state.selection = Selection::for_update(
            transaction.plane_input(state.plane())?,
            old.framebuffer(),
            state.framebuffer(),
        );
        let current = transaction.drm_dev().authority.snapshot();
        state.owner = resolve_owner(old, &state, current.as_ref());
        Ok(())
    }

    fn prepare_framebuffer(
        mut state: plane::PlaneStateMutator<'_, plane::PlaneState<PlaneState>>,
    ) -> Result {
        state.producer = None;
        if let Some(framebuffer) = state.framebuffer() {
            let dependencies = framebuffer::dependencies::Dependencies::acquire(
                framebuffer,
                state.producer_fence(),
            )?;
            let completion = dependencies.completion()?;
            let dependencies = Arc::new(dependencies, GFP_KERNEL)?;
            state.set_producer_fence(completion);
            state.producer = Some(dependencies);
        }
        Ok(())
    }
}

impl PlaneState {
    fn scene(state: &plane::PlaneState<Self>) -> Option<scene::Scene> {
        state
            .geometry
            .zip(state.content)
            .and_then(|(geometry, content)| {
                state.framebuffer().map(|framebuffer| {
                    scene::Scene::new(
                        framebuffer.to_owned_ref(),
                        geometry,
                        content,
                        state.owner.clone(),
                        state.producer.clone(),
                    )
                })
            })
    }
}

impl CrtcState {
    fn resolve_blank_owner(
        transaction: &atomic::AtomicStateComposer<Driver>,
        old: &crtc::CrtcState<Self>,
        state: &mut crtc::CrtcStateMutator<'_, crtc::CrtcState<Self>>,
    ) {
        // Native atomic validation checks plane visibility before invoking CRTC checks.
        state.visible = state.active()
            && transaction
                .get_new_plane_state(state.crtc().primary_plane())
                .map_or(old.visible, |plane| plane.geometry.is_some());
        state.blank_owner = if !state.active() || state.visible {
            None
        } else if !old.active() || old.visible || state.mode_changed() {
            transaction.drm_dev().authority.snapshot()
        } else {
            old.blank_owner.clone()
        };
    }

    fn check_configuration(
        old: &crtc::CrtcState<Self>,
        state: &mut crtc::CrtcStateMutator<'_, crtc::CrtcState<Self>>,
    ) -> Result {
        if !state.active() {
            state.configuration = None;
            return Ok(());
        }
        let connectors = state.connector_mask();
        let dimensions = [
            u32::from(state.mode().hdisplay()),
            u32::from(state.mode().vdisplay()),
        ];
        state.configuration = match old.configuration.as_ref() {
            Some(configuration)
                if old.active()
                    && !state.mode_changed()
                    && configuration.connector_mask() == connectors
                    && configuration.dimensions() == dimensions =>
            {
                Some(configuration.clone())
            }
            _ => Some(scene::Configuration::new(connectors, dimensions)?),
        };
        Ok(())
    }
}

#[vtable]
impl crtc::DriverCrtc for Crtc {
    type Args = ();
    type Driver = Driver;
    type State = CrtcState;
    type VblankImpl = vblank::SoftwareVblank<Self>;

    fn new(_: &Device<Driver>, _: &()) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {})
    }

    fn atomic_check(check: crtc::CrtcAtomicCheck<'_, Self>) -> Result {
        let (transaction, old, mut state) = check.take_all();
        CrtcState::resolve_blank_owner(transaction, old, &mut state);
        CrtcState::check_configuration(old, &mut state)
    }

    fn atomic_enable(commit: crtc::CrtcAtomicCommit<'_, Self>) {
        commit.crtc().vblank_on();
    }

    fn atomic_disable(commit: crtc::CrtcAtomicCommit<'_, Self>) {
        commit.crtc().vblank_off();
    }

    fn atomic_flush(mut commit: crtc::CrtcAtomicCommit<'_, Self>) {
        // A tick between publication and arming may delay notification, but cannot announce
        // a scene before it is accepted. The timer itself never accesses scene pixels.
        Self::publish_scene(&commit);

        let crtc = commit.crtc();
        if let Some(event) = commit.get_pending_vblank_event() {
            if let Ok(reference) = crtc.vblank_get() {
                if event.arm(reference).is_ok() {
                    return;
                }
            } else {
                event.send();
                return;
            }
        }
        if let Some(event) = commit.get_pending_vblank_event() {
            event.send();
        }
    }
}

impl Crtc {
    fn publish_scene(commit: &crtc::CrtcAtomicCommit<'_, Self>) {
        let Some(source) = commit.preparation_source() else {
            commit.crtc().drm_dev().output.close();
            return;
        };
        let primary = commit.crtc().primary_plane();
        let transaction = commit.atomic_state();
        let (old, state) = commit.old_new_state();
        let update = if !state.active() {
            SceneUpdate::Replace(None)
        } else if !state.visible {
            SceneUpdate::Replace(Some(scene::Scene::blank(state.blank_owner.clone())))
        } else {
            match transaction.get_new_plane_state(primary) {
                Some(plane) => SceneUpdate::Replace(PlaneState::scene(plane)),
                None => SceneUpdate::Retain,
            }
        };
        transaction.drm_dev().output.publish_with_configuration(
            source,
            update,
            state.configuration.clone(),
        );
        if old.configuration != state.configuration {
            if let Some(configuration) = &old.configuration {
                transaction
                    .drm_dev()
                    .capture_streams
                    .revoke_configuration(configuration);
            }
        }
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
    type Args = Arc<monitor::Monitor>;
    type Driver = Driver;
    type State = ConnectorState;

    fn new(_: &Device<Driver>, monitor: Self::Args) -> impl PinInit<Self, Error> {
        try_pin_init!(Self { monitor })
    }

    fn detect(connector: &connector::Connector<Self>, _: bool) -> connector::Status {
        connector.monitor.status()
    }

    fn get_modes<'a>(
        connector: connector::ConnectorGuard<'a, Self>,
        _: &ModeConfigGuard<'a, Driver>,
    ) -> i32 {
        connector.monitor.get_modes(&connector)
    }
}

#[vtable]
impl KmsDriver for Driver {
    fn create_capture_grant(
        dev: &Device<Self, kernel::drm::device::Registered>,
        _: &Self::RegistrationData<'_>,
        file: &kernel::drm::file::File<Self::File>,
        target: kernel::drm::capture::Target,
    ) -> Result<kernel::drm::capture::FilePair> {
        crate::file::File::create_capture_files(dev, file, target)
    }

    type FramebufferData = super::provenance::Provenance;
    fn framebuffer_data(
        _: &Device<Self>,
        file: Option<&kernel::drm::file::File<Self::File>>,
    ) -> Result<Self::FramebufferData> {
        Ok(super::provenance::Provenance::from_snapshot(
            file.and_then(|file| file.master_snapshot()),
        ))
    }
    type Connector = Connector;
    type Plane = Plane;
    type Crtc = Crtc;
    type Encoder = Encoder;

    fn mode_config_info(
        _: &device::Device,
        _: &<Self as kernel::drm::Driver>::Data,
    ) -> Result<ModeConfigInfo> {
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
        dev.enable_preparation(8)?;
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
        let connector = connector::UnregisteredConnector::<Connector>::new(
            dev,
            connector::Type::Virtual,
            dev.monitor.clone(),
        )?;
        connector.attach_edid_property();
        dev.execution.attach(connector)?;
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
        tail.commit_hw_done(enabled, planes)
    }
}
