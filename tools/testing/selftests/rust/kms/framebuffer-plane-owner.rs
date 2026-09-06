// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0308\]
#![no_std]

use kernel::drm::{
    gem::{shmem::Object, DriverObject},
    kms::{framebuffer::FramebufferPlane, KmsDriver},
};

pub fn nominated<D, T>(object: &Object<T>) -> FramebufferPlane<'_, D>
where
    D: KmsDriver<Object = Object<T>>,
    T: DriverObject<Driver = D>,
{
    FramebufferPlane {
        object,
        pitch: 256,
        offset: 0,
    }
}

#[cfg(negative)]
pub fn arbitrary<D, T>(object: &Object<T>) -> FramebufferPlane<'_, D>
where
    D: KmsDriver,
    T: DriverObject<Driver = D>,
{
    FramebufferPlane {
        object,
        pitch: 256,
        offset: 0,
    }
}
