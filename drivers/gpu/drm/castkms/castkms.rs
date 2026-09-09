// SPDX-License-Identifier: GPL-2.0-only

//! CastKMS virtual display device.

mod authority;
mod device;
mod display;
mod gem;
mod output;
mod provenance;
mod scene;

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
        let parent = faux::Registration::new(c"castkms", None)?;
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
struct File;

impl drm::file::DriverFile for File {
    type Driver = Driver;

    fn open(_: &drm::Device<Driver>) -> Result<Pin<KBox<Self>>> {
        Ok(KBox::new(Self, GFP_KERNEL)?.into())
    }
}

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
    const IOCTLS: &'static [drm::ioctl::DrmIoctlDescriptor] = &[];

    fn master_changed(dev: &drm::Device<Self>, master: Option<drm::auth::MasterRef<Self>>) {
        dev.authority.changed(master);
    }
}
