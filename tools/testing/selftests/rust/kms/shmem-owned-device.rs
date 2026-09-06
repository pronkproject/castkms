// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0277\]
#![no_std]

use kernel::drm::gem::{shmem::Object, DriverObject, ObjectRef};

pub fn owned<T: DriverObject>(object: &Object<T>) -> ObjectRef<Object<T>> {
    object.into()
}

#[cfg(negative)]
pub fn without_device<T: DriverObject>(object: &Object<T>) -> kernel::sync::aref::ARef<Object<T>> {
    object.into()
}
