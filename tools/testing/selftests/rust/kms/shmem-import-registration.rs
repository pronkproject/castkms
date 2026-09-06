// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0308\]
#![no_std]

use kernel::{
    dma_buf::DmaBuf,
    drm::{
        gem::{shmem::Object, DriverObject, ObjectRef},
        Device, Driver, Registered,
    },
    prelude::*,
};

pub fn registered<D, T>(
    dev: &Device<D, Registered>,
    buffer: &DmaBuf,
) -> Result<ObjectRef<Object<T>>>
where
    D: Driver<Object = Object<T>>,
    T: DriverObject<Driver = D>,
{
    Object::<T>::import(dev, buffer)
}

#[cfg(negative)]
pub fn unguarded<D, T>(dev: &Device<D>, buffer: &DmaBuf) -> Result<ObjectRef<Object<T>>>
where
    D: Driver<Object = Object<T>>,
    T: DriverObject<Driver = D>,
{
    Object::<T>::import(dev, buffer)
}
