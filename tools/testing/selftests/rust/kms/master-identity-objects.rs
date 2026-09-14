// SPDX-License-Identifier: GPL-2.0 OR MIT
// error-pattern: lifetime may not live long enough
#![no_std]

use kernel::drm::{
    auth::MasterRef,
    kms::KmsDriver, //
};

pub fn identity<T: KmsDriver>(master: &MasterRef<T>) -> Option<MasterRef<T>> {
    let identity = master.lock_current_identity()?;
    #[cfg(negative)]
    let borrowed = identity.with_objects(|objects| objects);
    #[cfg(negative)]
    return Some(borrowed.master().clone());

    #[cfg(not(negative))]
    Some(identity.with_objects(|objects| objects.master().clone()))
}
