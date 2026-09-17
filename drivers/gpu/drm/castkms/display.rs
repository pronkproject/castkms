// SPDX-License-Identifier: GPL-2.0-only

//! Virtual display pipelines and their accepted display descriptions.

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
use core::sync::atomic::{AtomicU32, Ordering};
use kernel::{
    device,
    drm::{
        fourcc,
        kms::*,
        Device, //
    },
    prelude::*,
    sync::{
        Arc,
        SetOnce, //
    }, //
};
use plane::{RawPlane, RawPlaneState};

#[pin_data]
pub(super) struct Plane {
    kind: scene::Kind,
}
#[pin_data]
pub(super) struct Crtc {
    pub(super) display: Arc<super::device::Display>,
    transition_property: AtomicU32,
    allocation_topology: SetOnce<Arc<super::execution::constraints::Topology>>,
}
#[pin_data]
pub(super) struct Encoder {}
#[pin_data]
pub(super) struct Connector {
    pub(super) monitor: Arc<monitor::Monitor>,
}

pub(super) struct ConnectorState;

pub(super) struct CrtcState {
    binding: Option<super::execution::constraints::backend::Binding>,
    transition: u64,
    transition_origin: Option<scene::Configuration>,
    // Complete atomic metadata, independent of commit-tail publication and producer waits.
    checked_scene: Option<scene::Scene>,
    output_color: Option<Arc<crate::color::OutputColor>>,
    configuration: Option<scene::Configuration>,
    visible: bool,
    layer_mask: u32,
    content: Option<scene::ContentSerial>,
    blank_owner: Option<kernel::drm::auth::MasterRef<Driver>>,
}

pub(super) struct PlaneState {
    color: Option<Arc<crate::color::Pipeline>>,
    geometry: Option<scene::Geometry>,
    selection: Selection,
    owner: Option<kernel::drm::auth::MasterRef<Driver>>,
    producer: Option<Arc<framebuffer::dependencies::Dependencies>>,
    prepared: Option<Arc<scene::Primary>>,
}

impl plane::DriverPlaneState for PlaneState {
    type Plane = Plane;
    fn new(_: &plane::Plane<Plane>) -> Result<Self> {
        Ok(Self {
            color: None,
            geometry: None,
            selection: Selection::RetainedFramebuffer,
            owner: None,
            producer: None,
            prepared: None,
        })
    }
    fn duplicate(&self) -> Result<Self> {
        Ok(Self {
            color: self.color.clone(),
            geometry: None,
            selection: Selection::RetainedFramebuffer,
            owner: self.owner.clone(),
            producer: None,
            prepared: None,
        })
    }
}

impl crtc::DriverCrtcState for CrtcState {
    type Crtc = Crtc;
    fn new(_: &crtc::Crtc<Crtc>) -> Result<Self> {
        Ok(Self {
            binding: None,
            transition: 0,
            transition_origin: None,
            output_color: None,
            checked_scene: None,
            configuration: None,
            visible: false,
            layer_mask: 0,
            content: None,
            blank_owner: None,
        })
    }
    fn duplicate(&self) -> Result<Self> {
        Ok(Self {
            binding: self.binding.clone(),
            // A transition tag belongs to one request, not subsequent animation.
            transition: 0,
            transition_origin: None,
            output_color: self.output_color.clone(),
            checked_scene: self.checked_scene.clone(),
            configuration: self.configuration.clone(),
            visible: self.visible,
            layer_mask: self.layer_mask,
            content: self.content,
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
        state.atomic_helper_check_scaled(&crtc_state, 1 << 12, 1 << 20, true, true)?;
        if crtc_state.active() && state.visible() {
            state.geometry = Some(scene::Geometry {
                source: [
                    state.source_x_16_16(),
                    state.source_y_16_16(),
                    state.source_width_16_16(),
                    state.source_height_16_16(),
                ],
                destination: [state.crtc_w(), state.crtc_h()],
                position: [state.crtc_x(), state.crtc_y()],
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

/// Retain layer metadata without mapping storage or claiming a source read.
fn describe_plane<S>(
    state: &S,
    producer: Option<Arc<framebuffer::dependencies::Dependencies>>,
) -> Result<Option<Arc<scene::Primary>>>
where
    S: RawPlaneState<Plane = plane::Plane<Plane>> + core::ops::Deref<Target = PlaneState>,
{
    let Some(geometry) = state.geometry else {
        return Ok(None);
    };
    Ok(Some(Arc::new(
        scene::Primary {
            yuv: state.yuv_color()?,
            color: state.color.clone(),
            framebuffer: state.framebuffer().ok_or(EINVAL)?.to_owned_ref(),
            geometry,
            producer,
            owner: state.owner.clone(),
            kind: state.plane().kind,
            zpos: state.zpos(),
        },
        GFP_KERNEL,
    )?))
}

#[vtable]
impl plane::DriverPlane for Plane {
    type Args = scene::Kind;
    type Driver = Driver;
    type State = PlaneState;

    fn new(_: &Device<Driver>, kind: scene::Kind) -> impl PinInit<Self, Error> {
        try_pin_init!(Self { kind })
    }

    fn format_modifier_supported(&self, _: u32, modifier: u64) -> bool {
        // The generic framebuffer representation bounds storage metadata. Actual
        // format/modifier eligibility belongs to the complete-scene contract.
        modifier != fourcc::FORMAT_MOD_INVALID
    }

    fn atomic_check(check: plane::PlaneAtomicCheck<'_, Self>) -> Result {
        let (transaction, old, mut state) = check.take_all();
        check_geometry(transaction, &mut state)?;
        state.color = crate::color::Pipeline::new(state.color_pipeline_snapshot(4)?)?;
        if state.geometry.is_some() {
            if state.plane().kind == scene::Kind::Cursor {
                let framebuffer = state.framebuffer().ok_or(EINVAL)?;
                if framebuffer.width() > super::execution::potential::MAX_CURSOR_DIMENSION
                    || framebuffer.height() > super::execution::potential::MAX_CURSOR_DIMENSION
                {
                    return Err(EINVAL);
                }
            }
        }
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
        state.prepared = None;
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
        state.prepared = describe_plane(&state, state.producer.clone())?;
        Ok(())
    }
}

impl CrtcState {
    pub(crate) fn tag_transition(&mut self, token: u64) {
        self.transition = token;
    }

    fn validation_update(&self) -> Result<crate::execution::coordinator::Update<'_>> {
        Ok(crate::execution::coordinator::Update {
            scene: self.validation_view()?,
            token: self.transition,
            previous_configuration: self.transition_origin.as_ref(),
            configuration: self.configuration.as_ref(),
        })
    }

    fn validation_view(&self) -> Result<crate::execution::validation::SceneView<'_>> {
        match (&self.checked_scene, &self.configuration) {
            (Some(scene), Some(configuration)) => Ok(crate::execution::validation::SceneView::Enabled {
                scene,
                output: configuration.dimensions(),
            }),
            (None, None) => Ok(crate::execution::validation::SceneView::Disabled),
            _ => Err(EINVAL),
        }
    }

    /// Proposed/installed metadata only, never a published source or producer-wait proof.
    #[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
    pub(crate) fn checked_scene(&self) -> Option<&scene::Scene> {
        self.checked_scene.as_ref()
    }

    fn describe_scene(
        transaction: &atomic::AtomicStateComposer<Driver>,
        old: &crtc::CrtcState<Self>,
        state: &mut crtc::CrtcStateMutator<'_, crtc::CrtcState<Self>>,
    ) -> Result {
        if !state.active() {
            state.checked_scene = None;
            return Ok(());
        }
        let mut scene = if state.visible {
            old.checked_scene
                .clone()
                .unwrap_or_else(|| scene::Scene::blank(None))
        } else {
            scene::Scene::blank(state.blank_owner.clone())
        };
        let mut result: Result = Ok(());
        transaction.try_for_each_new_plane_state(|plane, opaque| {
            if result.is_err() {
                return;
            }
            result = (|| {
                let layer = if state.layer_mask & plane.mask() != 0 {
                    // Producer dependencies are acquired only by framebuffer preparation.
                    describe_plane(plane::PlaneState::<PlaneState>::from_opaque(opaque), None)?
                } else {
                    None
                };
                scene.set_layer(plane.index() as usize, layer);
                Ok(())
            })();
        })?;
        result?;
        if scene.layers().count() != state.layer_mask.count_ones() as usize {
            return Err(EINVAL);
        }
        scene.finalize(state.content);
        scene.output_color = state.output_color.clone();
        scene.set_binding(state.binding.as_deref());
        state.checked_scene = Some(scene);
        Ok(())
    }

    fn resolve_blank_owner(
        transaction: &atomic::AtomicStateComposer<Driver>,
        old: &crtc::CrtcState<Self>,
        state: &mut crtc::CrtcStateMutator<'_, crtc::CrtcState<Self>>,
    ) -> Result {
        // Native atomic validation checks plane visibility before invoking CRTC checks.
        let mut mask = old.layer_mask;
        let mut changed =
            state.mode_changed() || state.color_mgmt_changed() || state.active() != old.active();
        transaction.try_for_each_new_plane_state(|plane, opaque| {
            changed |= old.layer_mask & plane.mask() != 0;
            mask &= !plane.mask();
            let plane_state = plane::PlaneState::<PlaneState>::from_opaque(opaque);
            if plane_state.geometry.is_some()
                && plane_state
                    .crtc()
                    .is_some_and(|crtc| crtc.index() == state.crtc().index())
            {
                mask |= plane.mask();
                changed = true;
            }
        })?;
        state.layer_mask = if state.active() { mask } else { 0 };
        state.visible = state.layer_mask != 0;
        state.content = scene::ContentSerial::for_update(old.content, state.visible && changed)?;
        state.blank_owner = if !state.active() || state.visible {
            None
        } else if !old.active() || old.visible || state.mode_changed() {
            transaction.drm_dev().authority.snapshot()
        } else {
            old.blank_owner.clone()
        };
        Ok(())
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
        let refresh_millihz = state.mode().vrefresh_millihz()?;
        let mode_flags = state.mode().flags().bits();
        state.configuration = match old.configuration.as_ref() {
            Some(configuration)
                if old.active()
                    && !state.mode_changed()
                    && configuration.connector_mask() == connectors
                    && configuration.dimensions() == dimensions =>
            {
                Some(configuration.clone())
            }
            _ => Some(scene::Configuration::new(
                connectors,
                dimensions,
                refresh_millihz,
                mode_flags,
            )?),
        };
        Ok(())
    }
}

#[vtable]
impl crtc::DriverCrtc for Crtc {
    type Args = Arc<super::device::Display>;
    type Driver = Driver;
    type State = CrtcState;
    type VblankImpl = vblank::SoftwareVblank<Self>;

    fn new(_: &Device<Driver>, display: &Self::Args) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {
            display: display.clone(),
            transition_property: AtomicU32::new(0),
            allocation_topology: SetOnce::new(),
        })
    }

    fn atomic_set_property(&self, state: &mut CrtcState, property: u32, value: u64) -> Result {
        if property != self.transition_property.load(Ordering::Relaxed) {
            return Err(EINVAL);
        }
        state.tag_transition(value);
        Ok(())
    }

    fn atomic_get_property(&self, _: &CrtcState, property: u32) -> Result<u64> {
        if property != self.transition_property.load(Ordering::Relaxed) {
            return Err(EINVAL);
        }
        // Request-only input: neither state duplication nor readback renews a tag.
        Ok(0)
    }

    fn atomic_check(check: crtc::CrtcAtomicCheck<'_, Self>) -> Result {
        let (transaction, old, mut state) = check.take_all();
        state.transition_origin = old.configuration.clone();
        CrtcState::resolve_blank_owner(transaction, old, &mut state)?;
        state.validate_color_mgmt(256)?;
        state.output_color =
            crate::color::OutputColor::new(state.degamma_lut(), state.ctm(), state.gamma_lut())?;
        CrtcState::check_configuration(old, &mut state)?;
        if let Some(provider) = state.crtc().display.constraints.as_ref() {
            let entry = state.constraints_entry().ok_or(EINVAL)?;
            state.binding = if old.binding.as_ref().is_some_and(|binding|
                core::ptr::eq(&***binding, entry))
            {
                old.binding.clone()
            } else if !state.enabled() && !state.active() && state.plane_mask() == 0
                && old.constraints_entry().is_some_and(|old| core::ptr::eq(old, entry))
            {
                None
            } else {
                Some(provider.resolve(entry)?)
            };
        }
        CrtcState::describe_scene(transaction, old, &mut state)?;
        if transaction.drm_dev().constraints_enabled {
            return if state.transition == 0 { Ok(()) } else { Err(EOPNOTSUPP) };
        }
        let mut updates = core::array::from_fn(|_| None);
        *updates.get_mut(state.crtc().index() as usize).ok_or(EINVAL)? =
            Some(state.validation_update()?);
        let mut validation = transaction.drm_dev().validation.lock();
        drop(validation.prepare(&updates)?);
        Ok(())
    }

    fn atomic_enable(commit: crtc::CrtcAtomicCommit<'_, Self>) {
        commit.crtc().vblank_on();
        #[cfg(CONFIG_DRM_CASTKMS_AUDIO)]
        commit.crtc().display.monitor.audio_link.set_enabled(true);
    }

    fn atomic_disable(commit: crtc::CrtcAtomicCommit<'_, Self>) {
        #[cfg(CONFIG_DRM_CASTKMS_AUDIO)]
        commit.crtc().display.monitor.audio_link.set_enabled(false);
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
    #[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
    pub(crate) fn allocation_topology(&self) -> Result<&super::execution::constraints::Topology> {
        self.allocation_topology.as_ref().map(|topology| &**topology).ok_or(ENODEV)
    }

    fn publish_scene(commit: &crtc::CrtcAtomicCommit<'_, Self>) {
        let Some(source) = commit.preparation_source() else {
            commit.crtc().display.output.close();
            return;
        };
        let transaction = commit.atomic_state();
        let (old, state) = commit.old_new_state();
        let update = if !state.active() {
            SceneUpdate::Replace(None)
        } else if !state.visible {
            let mut scene = scene::Scene::blank(state.blank_owner.clone());
            scene.output_color = state.output_color.clone();
            scene.set_binding(state.binding.as_deref());
            SceneUpdate::Replace(Some(scene))
        } else if old.visible && state.content == old.content
            && old.constraints_entry().map(core::ptr::from_ref)
                == state.constraints_entry().map(core::ptr::from_ref)
        {
            SceneUpdate::Retain
        } else {
            let mut scene = commit
                .crtc()
                .display
                .output
                .with_accepted(|accepted| accepted.and_then(|accepted| accepted.scene.cloned()))
                .unwrap_or_else(|| scene::Scene::blank(None));
            transaction.for_each_new_plane_state(|plane, opaque| {
                let plane_state = plane::PlaneState::<PlaneState>::from_opaque(opaque);
                let layer = if state.layer_mask & plane.mask() != 0 {
                    plane_state.prepared.clone()
                } else {
                    None
                };
                scene.set_layer(plane.index() as usize, layer);
            });
            scene.finalize(state.content);
            scene.output_color = state.output_color.clone();
            scene.set_binding(state.binding.as_deref());
            SceneUpdate::Replace(Some(scene))
        };
        commit.crtc().display.output.publish_with_configuration(
            source,
            update,
            state.configuration.clone(),
        );
        if old.configuration != state.configuration {
            // A tagged migration deliberately changes the candidate's configuration.
            // The gate, not an unchanged mode identity, controls its activation.
            if state.transition == 0 {
                commit.crtc().display.startup.configuration_changed();
            }
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

fn install_constraints<'a>(
    install: atomic::Install<'a, Driver>,
) -> atomic::InstallResult<'a, Driver> {
    let mut selected = None;
    let mut result = Ok(());
    install.state().for_each_new_crtc_state(|crtc, opaque| {
        if result.is_err() {
            return;
        }
        result = (|| {
            let state = crtc::CrtcState::<CrtcState>::from_opaque(opaque);
            let old = install.state().get_old_crtc_state(crtc).ok_or(EINVAL)?;
            if !state.enabled() && !state.active() && state.plane_mask() == 0
                && state.constraints_entry().map(core::ptr::from_ref)
                    == old.constraints_entry().map(core::ptr::from_ref)
            {
                return Ok(());
            }
            let entry = crtc.display.constraints.as_ref().ok_or(EOPNOTSUPP)?
                .resolve(state.constraints_entry().ok_or(EINVAL)?)?;
            let renderer = matches!(&*entry.backend(),
                super::execution::constraints::backend::Backend::Renderer(_));
            if renderer {
                if selected.is_some() {
                    return Err(EOPNOTSUPP);
                }
                selected = Some(entry);
            }
            Ok(())
        })();
    });
    if let Err(error) = result {
        return install.reject(error);
    }
    if let Some(entry) = selected {
        let backend = entry.backend();
        let _ready = match backend.hold_ready() {
            Ok(guard) => guard,
            Err(error) => return install.reject(error),
        };
        install.install()
    } else {
        install.install()
    }
}

#[vtable]
impl KmsDriver for Driver {
    fn constraints_check(
        transaction: &atomic::AtomicStateReader<Self>,
        state: &crtc::OpaqueCrtcState<Self>,
        entry: &kernel::drm::constraints::OpaqueEntry,
    ) -> Result {
        let state = crtc::CrtcState::<CrtcState>::from_opaque(state);
        state.crtc().display.constraints.as_ref().ok_or(EOPNOTSUPP)?
            .check(entry, transaction.drm_dev().authority.interval().ok(), state.validation_view()?)
    }

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
            max_resolution: (super::execution::potential::MAX_DIMENSION,
                super::execution::potential::MAX_DIMENSION),
            max_cursor: (
                super::execution::potential::MAX_CURSOR_DIMENSION,
                super::execution::potential::MAX_CURSOR_DIMENSION,
            ),
            preferred_depth: 24,
            preferred_fourcc: Some(fourcc::XRGB8888),
            enable_default_client: false,
        })
    }

    fn create_objects(dev: &UnregisteredKmsDevice<'_, Self>) -> Result {
        dev.enable_preparation(8)?;
        let domain = if dev.constraints_enabled {
            Some(dev.enable_constraints((dev.displays.len()
                * super::execution::constraints::provider::CAPACITY) as u32)?)
        } else {
            None
        };
        let mut allocations = KVec::with_capacity(dev.displays.len(), GFP_KERNEL)?;
        for (index, display) in dev.displays.iter().enumerate() {
            let mut allocation_planes = KVec::new();
            let plane = plane::UnregisteredPlane::<Plane>::new(
                dev,
                0,
                &super::execution::potential::FORMATS,
                Some(&[fourcc::FORMAT_MOD_LINEAR]),
                plane::Type::Primary,
                None,
                scene::Kind::Primary,
            )?;
            plane.create_zpos_immutable_property(0)?;
            plane.create_blend_mode_property(plane::BlendModes::PREMULTIPLIED)?;
            plane.create_yuv_color_properties()?;
            plane.create_nearest_scaling_filter_property()?;
            if dev.enable_plane_pipeline {
                plane.create_srgb_matrix_pipeline()?;
            }
            let cursor = if dev.enable_cursor {
                let cursor = plane::UnregisteredPlane::<Plane>::new(
                    dev,
                    1 << index,
                    &[fourcc::ARGB8888],
                    Some(&[fourcc::FORMAT_MOD_LINEAR]),
                    plane::Type::Cursor,
                    None,
                    scene::Kind::Cursor,
                )?;
                cursor.create_zpos_immutable_property(31)?;
                cursor.create_yuv_color_properties()?;
                cursor.create_nearest_scaling_filter_property()?;
                if dev.enable_plane_pipeline {
                    cursor.create_srgb_matrix_pipeline()?;
                }
                cursor.create_blend_mode_property(plane::BlendModes::PREMULTIPLIED)?;
                Some(cursor)
            } else {
                None
            };
            let crtc =
                crtc::UnregisteredCrtc::<Crtc>::new(dev, plane, cursor, None, display.clone())?;
            allocation_planes.push(
                super::execution::constraints::Plane {
                    id: plane.object_id(),
                    kind: scene::Kind::Primary,
                },
                GFP_KERNEL,
            )?;
            if let Some(cursor) = cursor {
                allocation_planes.push(
                    super::execution::constraints::Plane {
                        id: cursor.object_id(),
                        kind: scene::Kind::Cursor,
                    },
                    GFP_KERNEL,
                )?;
            }
            allocations.push((crtc, allocation_planes), GFP_KERNEL)?;
            if domain.is_none() {
                let transition = crtc.attach_replayable_range_property(
                    c"CASTKMS_TRANSITION", 0, u64::MAX, 0,
                )?;
                crtc.transition_property.store(transition, Ordering::Relaxed);
            }
            crtc.enable_color_mgmt(256, true, 256);
            crtc.set_gamma_size(256)?;
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
                display.monitor.clone(),
            )?;
            connector.attach_edid_property();
            display.execution.attach(connector)?;
            connector.attach_encoder(encoder)?;
        }
        if dev.enable_overlay {
            for _ in 0..8 {
                let plane = plane::UnregisteredPlane::<Plane>::new(
                    dev,
                    (1 << dev.displays.len()) - 1,
                    &super::execution::potential::FORMATS,
                    Some(&[fourcc::FORMAT_MOD_LINEAR]),
                    plane::Type::Overlay,
                    None,
                    scene::Kind::Overlay,
                )?;
                plane.create_zpos_property(1, 1, 30)?;
                plane.create_yuv_color_properties()?;
                plane.create_nearest_scaling_filter_property()?;
                if dev.enable_plane_pipeline {
                    plane.create_srgb_matrix_pipeline()?;
                }
                plane.create_blend_mode_property(plane::BlendModes::PREMULTIPLIED)?;
                for (_, allocation_planes) in &mut allocations {
                    allocation_planes.push(
                        super::execution::constraints::Plane {
                            id: plane.object_id(),
                            kind: scene::Kind::Overlay,
                        },
                        GFP_KERNEL,
                    )?;
                }
            }
        }
        for (crtc, planes) in allocations {
            let topology = Arc::new(
                super::execution::constraints::Topology::new(planes)?, GFP_KERNEL,
            )?;
            if let Some(domain) = &domain {
                let provider = super::execution::constraints::provider::Provider::new(
                    domain.clone(), crtc.object_id(), crtc.display.output.identity().clone(),
                    topology.clone(),
                )?;
                dev.attach_constraints(&crtc, provider.initial(),
                    super::execution::constraints::provider::CAPACITY as u32)?;
                if !crtc.display.constraints.populate(provider) {
                    return Err(EEXIST);
                }
            }
            if !crtc.allocation_topology.populate(topology) {
                return Err(EEXIST);
            }
        }
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
        tail.commit_hw_done(enabled, planes)
    }

    fn atomic_commit_install<'a>(install: atomic::Install<'a, Self>) -> atomic::InstallResult<'a, Self> {
        if install.state().drm_dev().constraints_enabled {
            return install_constraints(install);
        }
        let device_state = core::ops::Deref::deref(install.state().drm_dev()).clone();
        let mut updates = core::array::from_fn(|_| None);
        let mut result = Ok(());
        install.state().for_each_new_crtc_state(|crtc, opaque| {
            if result.is_ok() {
                let state = crtc::CrtcState::<CrtcState>::from_opaque(opaque);
                result = state.validation_update().and_then(|update| {
                    *updates.get_mut(crtc.index() as usize).ok_or(EINVAL)? = Some(update);
                    Ok(())
                });
            }
        });
        if let Err(error) = result {
            return install.reject(error);
        }
        let mut validation = device_state.validation.lock();
        let prepared = match validation.prepare(&updates) {
            Ok(prepared) => prepared,
            Err(error) => return install.reject(error),
        };
        let mut retired = None;
        let result = install.install_then(|| retired = Some(prepared.commit()));
        drop(validation);
        drop(retired);
        result
    }
}
