// SPDX-License-Identifier: GPL-2.0-only

//! CastKMS virtual display device.

mod audio;
mod authority;
mod capture;
mod color;
mod device;
mod display;
mod display_control;
mod execution;
mod formats;
mod file;
mod gem;
mod host_compositor;
mod image_access;
mod image_storage;
mod monitor;
mod monitor_file;
mod output;
mod provenance;
mod renderer;
mod renderer_file;
mod scene;

use file::File;

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
mod tests;

use kernel::{
    device as bus,
    drm,
    faux,
    prelude::*,
    sync::Arc, //
};

module! {
    type: CastKms,
    name: "castkms",
    authors: ["Ray Strode"],
    description: "CastKMS virtual display",
    license: "GPL",
    imports_ns: ["DMA_BUF"],
    params: {
        enable_cursor: bool {
            default: true,
            description: "Enable cursor planes",
        },
        enable_overlay: bool {
            default: true,
            description: "Enable eight shared overlay planes",
        },
        enable_plane_pipeline: bool {
            default: true,
            description: "Enable per-plane sRGB and matrix color pipelines",
        },
        max_outputs: u32 {
            default: 8,
            description: "Number of virtual display outputs (1-8)",
        },
    },
}

// Fields are private and ordered so DRM unplug and atomic shutdown finish before the faux
// parent unbinds. No registration data borrows module storage.
struct CastKms {
    _display: drm::Registration<'static, Driver>,
    _parent: faux::Registration,
    state: device::Owner,
}

impl Drop for CastKms {
    fn drop(&mut self) {
        // Close before atomic shutdown; an already accepted tail must not repopulate the output.
        self.state.close();
    }
}

impl kernel::Module for CastKms {
    fn init(_: &'static ThisModule) -> Result<Self> {
        let state = device::Owner::new_features(
            module_parameters::max_outputs.value(),
            module_parameters::enable_cursor.value(),
            module_parameters::enable_overlay.value(),
            module_parameters::enable_plane_pipeline.value(),
        )?;
        Self::new_with_state(c"castkms", state)
    }
}

impl CastKms {
    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    fn new(name: &CStr) -> Result<Self> {
        Self::new_outputs(name, 1)
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    fn new_outputs(name: &CStr, output_count: u32) -> Result<Self> {
        Self::new_features(name, output_count, true, false, false)
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    fn new_features(name: &CStr, output_count: u32, enable_cursor: bool, enable_overlay: bool, enable_plane_pipeline: bool) -> Result<Self> {
        let state = device::Owner::new_configuration(
            output_count, enable_cursor, enable_overlay, enable_plane_pipeline, false,
        )?;
        Self::new_with_state(name, state)
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    fn new_constraints(name: &CStr, output_count: u32) -> Result<Self> {
        Self::new_with_state(name, device::Owner::new_constraints(output_count)?)
    }

    fn new_with_state(name: &CStr, state: device::Owner) -> Result<Self> {
        let parent =
            faux::Registration::new_with_dma_mask(name, None, kernel::dma::DmaMask::new::<64>())?;
        let drm =
            drm::UnregisteredDevice::<Driver>::new(parent.as_ref(), Ok::<_, Error>(state.state()))?;
        // SAFETY: After successful construction, field drop order unplugs DRM before parent
        // unbind. On failure the registration constructor unwinds before the local parent drops.
        let display = unsafe {
            drm::Registration::new_static(parent.as_ref().as_ref(), drm, Ok::<(), Error>(()), 0)?
        };
        Ok(Self {
            _display: display,
            _parent: parent,
            state,
        })
    }
}

struct Driver;

type Output = output::Output<scene::Scene, Option<scene::Configuration>>;

#[vtable]
impl drm::Driver for Driver {
    const FEAT_CURSOR_HOTSPOT: bool = true;
    type Data = Arc<device::State>;
    type RegistrationData<'a> = ();
    type File = File;
    type Object = drm::gem::shmem::Object<gem::Object>;
    type ParentDevice<Ctx: bus::DeviceContext> = faux::Device<Ctx>;
    type Kms = Self;

    const INFO: drm::DriverInfo = drm::DriverInfo {
        major: 0,
        minor: 0,
        patchlevel: 0,
        name: c"castkms",
        desc: c"CastKMS virtual display",
    };
    kernel::declare_drm_ioctls! {
        (CASTKMS_CREATE_MONITOR_CONTROL, drm_castkms_create_monitor_control,
         drm::ioctl::MASTER, monitor_file::create),
        (CASTKMS_CREATE_RENDERER_CONTROL, drm_castkms_create_renderer_control,
         drm::ioctl::MASTER, renderer_file::create),
        (CASTKMS_CREATE_AUDIO_CAPTURE, drm_castkms_create_audio_capture,
         drm::ioctl::MASTER, audio::create),
    }

    fn master_changed(dev: &drm::Device<Self>, master: Option<drm::auth::MasterRef<Self>>) {
        dev.capture_streams.revoke_all();
        dev.renderer_workers.revoke_all();
        #[cfg(CONFIG_DRM_CASTKMS_AUDIO)]
        dev.audio_grants.suspend_all();
        dev.authority.changed(master);
        dev.changed.notify_all();
    }
}
