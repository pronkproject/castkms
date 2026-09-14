// SPDX-License-Identifier: GPL-2.0 OR MIT
// error-pattern: cannot move out of .blob. because it is borrowed
#![no_std]

use kernel::drm::kms::{
    blob::Blob,
    KmsDriver, //
};

pub fn length<D: KmsDriver>(blob: Blob<D>) -> usize {
    let bytes = blob.as_bytes();
    #[cfg(negative)]
    drop(blob);
    let length = bytes.len();
    #[cfg(not(negative))]
    drop(blob);
    length
}
