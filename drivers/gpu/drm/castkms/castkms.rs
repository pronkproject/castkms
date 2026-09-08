// SPDX-License-Identifier: GPL-2.0-only

//! CastKMS virtual display device.

mod display;
mod gem;
mod output;
mod provenance;
mod scene;

use kernel::{
    device,
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
    output: Arc<output::Output<scene::Scene>>,
}

impl Drop for CastKms {
    fn drop(&mut self) {
        // Close before atomic shutdown; an already accepted tail must not repopulate the output.
        self.output.close();
    }
}

impl kernel::Module for CastKms {
    fn init(_: &'static ThisModule) -> Result<Self> {
        let parent = faux::Registration::new(c"castkms", None)?;
        let output = Arc::pin_init(output::Output::new(), GFP_KERNEL)?;
        let drm = drm::UnregisteredDevice::<Driver>::new(
            parent.as_ref(),
            Ok::<_, Error>(output.clone()),
        )?;
        // SAFETY: After successful construction, field drop order unplugs DRM before parent
        // unbind. On failure the registration constructor unwinds before the local parent drops.
        let display = unsafe {
            drm::Registration::new_static(parent.as_ref().as_ref(), drm, Ok::<(), Error>(()), 0)?
        };
        Ok(Self {
            _display: display,
            _parent: parent,
            output,
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
    type Data = Arc<output::Output<scene::Scene>>;
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
