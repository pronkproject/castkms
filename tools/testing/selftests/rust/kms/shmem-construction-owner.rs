// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0271\]
#![no_std]

use kernel::{
    drm::{
        gem::{shmem::Object, DriverObject, ObjectRef},
        Device, Driver,
    },
    prelude::*,
};

pub fn nominated<D, T>(dev: &Device<D>, args: T::Args) -> Result<ObjectRef<Object<T>>>
where
    D: Driver<Object = Object<T>>,
    T: DriverObject<Driver = D>,
{
    Object::<T>::new(dev, 4096, Default::default(), args)
}

#[cfg(negative)]
pub fn arbitrary<T: DriverObject>(
    dev: &Device<T::Driver>,
    args: T::Args,
) -> Result<ObjectRef<Object<T>>> {
    Object::<T>::new(dev, 4096, Default::default(), args)
}
