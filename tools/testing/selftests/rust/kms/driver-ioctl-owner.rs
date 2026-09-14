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
    prelude::*, //
};

pub struct Replacement<D: drm::Driver>(PhantomData<fn() -> D>);
pub struct LocalFile<D: drm::Driver>(PhantomData<fn() -> D>);
pub struct LocalObject<D: drm::Driver>(PhantomData<fn() -> D>);

impl<D: drm::Driver> drm::file::DriverFile for LocalFile<D> {
    type Driver = Replacement<D>;
    fn open(_: &Device<Self::Driver>) -> Result<Pin<KBox<Self>>> {
        Ok(KBox::new(Self(PhantomData), GFP_KERNEL)?.into())
    }
}

#[vtable]
impl<D: drm::Driver> gem::DriverObject for LocalObject<D> {
    type OwnerModule = D::OwnerModule;
    type Driver = Replacement<D>;
    type Args = ();
    fn new(_: &Device<Self::Driver>, _: usize, _: ()) -> impl PinInit<Self, Error> {
        Ok(Self(PhantomData))
    }
}

#[vtable]
impl<D: drm::Driver> drm::Driver for Replacement<D> {
    type OwnerModule = D::OwnerModule;
    type Data = [u64; 8];
    type RegistrationData<'a> = ();
    type ParentDevice<C: device::DeviceContext> = D::ParentDevice<C>;
    type Kms = PhantomData<Self>;
    type File = LocalFile<D>;
    type Object = gem::Object<LocalObject<D>>;

    const INFO: drm::DriverInfo = D::INFO;
    #[cfg(negative)]
    const IOCTLS: &'static [drm::ioctl::DrmIoctlDescriptor<Self>] = D::IOCTLS;
    #[cfg(not(negative))]
    const IOCTLS: &'static [drm::ioctl::DrmIoctlDescriptor<Self>] = &[];
}
