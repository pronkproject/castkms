// SPDX-License-Identifier: GPL-2.0 OR MIT
// error-pattern: cannot be sent between threads safely
#![no_std]

use kernel::drm::{
    auth::{
        MasterIdentityGuard,
        MasterRef, //
    },
    kms::KmsDriver, //
};

pub fn identity<T: KmsDriver>() {
    fn send<U: Send>() {}
    #[cfg(negative)]
    send::<MasterIdentityGuard<'static, T>>();
    #[cfg(not(negative))]
    send::<MasterRef<T>>();
}
