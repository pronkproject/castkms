// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0271\]
#![no_std]

use core::marker::PhantomData;
use kernel::{
    device,
    drm::{self, gem, Device},
    prelude::*,
};

pub struct Replacement<D: drm::Driver + 'static>(PhantomData<fn() -> D>);
pub struct LocalFile<D: drm::Driver + 'static>(PhantomData<fn() -> D>);
pub struct LocalObject<D: drm::Driver + 'static>(PhantomData<fn() -> D>);

impl<D: drm::Driver + 'static> drm::file::DriverFile for LocalFile<D> {
    type Driver = Replacement<D>;
    fn open(_: &Device<Self::Driver>) -> Result<Pin<KBox<Self>>> {
        Ok(KBox::new(Self(PhantomData), GFP_KERNEL)?.into())
    }
}

#[vtable]
impl<D: drm::Driver + 'static> gem::DriverObject for LocalObject<D> {
    type OwnerModule = D::OwnerModule;
    type Driver = Replacement<D>;
    type Args = ();
    fn new(_: &Device<Self::Driver>, _: usize, _: ()) -> impl PinInit<Self, Error> {
        Ok(Self(PhantomData))
    }
}

#[vtable]
impl<D: drm::Driver + 'static> drm::Driver for Replacement<D> {
    type OwnerModule = D::OwnerModule;
    type Data = [u64; 8];
    type RegistrationData<'a> = ();
    type ParentDevice<C: device::DeviceContext> = D::ParentDevice<C>;
    type Kms = PhantomData<Self>;
    type File = LocalFile<D>;
    #[cfg(negative)]
    type Object = D::Object;
    #[cfg(not(negative))]
    type Object = gem::Object<LocalObject<D>>;

    const INFO: drm::DriverInfo = D::INFO;
    const IOCTLS: &'static [drm::ioctl::DrmIoctlDescriptor] = &[];
}
