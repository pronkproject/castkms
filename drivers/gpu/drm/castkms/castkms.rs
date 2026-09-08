// SPDX-License-Identifier: GPL-2.0-only

//! CastKMS virtual display device.

mod display;
mod gem;

use kernel::{
    device,
    drm,
    faux,
    prelude::*, //
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
}

impl kernel::Module for CastKms {
    fn init(_: &'static ThisModule) -> Result<Self> {
        let parent = faux::Registration::new(c"castkms", None)?;
        let drm = drm::UnregisteredDevice::<Driver>::new(parent.as_ref(), Ok::<(), Error>(()))?;
        // SAFETY: After successful construction, field drop order unplugs DRM before parent
        // unbind. On failure the registration constructor unwinds before the local parent drops.
        let display = unsafe {
            drm::Registration::new_static(parent.as_ref().as_ref(), drm, Ok::<(), Error>(()), 0)?
        };
        Ok(Self {
            _display: display,
            _parent: parent,
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
    type Data = ();
    type RegistrationData<'a> = ();
    type File = File;
    type Object = drm::gem::shmem::Object<gem::Object>;
    type ParentDevice<Ctx: device::DeviceContext> = faux::Device<Ctx>;
    type Kms = Self;

    const INFO: drm::DriverInfo = drm::DriverInfo {
        major: 0,
        minor: 0,
        patchlevel: 0,
        name: c"castkms",
        desc: c"CastKMS virtual display",
    };
    const IOCTLS: &'static [drm::ioctl::DrmIoctlDescriptor] = &[];
}
