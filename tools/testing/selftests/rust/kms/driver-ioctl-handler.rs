// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0308\]
#![no_std]

use core::marker::PhantomData;
use kernel::{
    device,
    drm::{
        self,
        gem,
        Device, //
    },
    faux,
    prelude::*, //
};

pub struct LocalModule;

impl kernel::ModuleMetadata for LocalModule {
    const NAME: &'static kernel::str::CStr = c"ioctl_typecheck";
    // SAFETY: This compile-only fixture represents built-in code, with no module pointer.
    const THIS_MODULE: kernel::ThisModule =
        unsafe { kernel::ThisModule::from_ptr(core::ptr::null_mut()) };
}

pub struct Original;
pub struct Replacement;
pub struct LocalFile<D: drm::Driver>(PhantomData<fn() -> D>);
pub struct LocalObject<D: drm::Driver>(PhantomData<fn() -> D>);

impl<D: drm::Driver<File = Self>> drm::file::DriverFile for LocalFile<D> {
    type Driver = D;
    fn open(_: &Device<D>) -> Result<Pin<KBox<Self>>> {
        Ok(KBox::new(Self(PhantomData), GFP_KERNEL)?.into())
    }
}

#[vtable]
impl<D: drm::Driver> gem::DriverObject for LocalObject<D> {
    type Driver = D;
    type Args = ();
    fn new(_: &Device<D>, _: usize, _: ()) -> impl PinInit<Self, Error> {
        Ok(Self(PhantomData))
    }
}

macro_rules! driver {
    ($driver:ident, $handler:ident) => {
        #[vtable]
        impl drm::Driver for $driver {
            type Data = ();
            type RegistrationData<'a> = ();
            type ParentDevice<C: device::DeviceContext> = faux::Device<C>;
            type Kms = PhantomData<Self>;
            type File = LocalFile<Self>;
            type Object = gem::Object<LocalObject<Self>>;

            const INFO: drm::DriverInfo = drm::DriverInfo {
                major: 0,
                minor: 0,
                patchlevel: 0,
                name: c"ioctl_typecheck",
                desc: c"Ioctl owner type check",
            };
            kernel::declare_drm_ioctls! {
                (NOVA_GETPARAM, drm_nova_getparam, 0, $handler),
            }
        }
    };
}

fn original(
    _: &Device<Original, drm::Registered>,
    _: &(),
    _: &mut kernel::uapi::drm_nova_getparam,
    _: &drm::File<LocalFile<Original>>,
) -> Result<u32> {
    Ok(0)
}

fn replacement(
    _: &Device<Replacement, drm::Registered>,
    _: &(),
    _: &mut kernel::uapi::drm_nova_getparam,
    _: &drm::File<LocalFile<Replacement>>,
) -> Result<u32> {
    Ok(0)
}

driver!(Original, original);
#[cfg(negative)]
driver!(Replacement, original);
#[cfg(not(negative))]
driver!(Replacement, replacement);
