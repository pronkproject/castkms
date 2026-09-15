// SPDX-License-Identifier: GPL-2.0-only

//! CastKMS virtual display device.

mod authority;
mod capture;
mod device;
mod display;
mod display_control;
mod execution;
mod file;
mod gem;
mod host_compositor;
mod host_snapshot;
mod image_access;
mod monitor;
mod monitor_file;
mod output;
mod provenance;
mod renderer;
mod renderer_file;
mod renderer_startup;
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
        Self::new(c"castkms")
    }
}

impl CastKms {
    fn new(name: &CStr) -> Result<Self> {
        let parent =
            faux::Registration::new_with_dma_mask(name, None, kernel::dma::DmaMask::new::<64>())?;
        let state = device::Owner::new()?;
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
    }

    fn master_changed(dev: &drm::Device<Self>, master: Option<drm::auth::MasterRef<Self>>) {
        dev.startup.cancel_current();
        dev.capture_streams.revoke_all();
        dev.authority.changed(master);
    }
}
