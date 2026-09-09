// SPDX-License-Identifier: GPL-2.0 OR MIT
// error-pattern: no method named `completion` found
#![no_std]

use kernel::{
    dma_fence::Fence,
    drm::preparation::RetirementSet,
    error::Result,
    sync::aref::ARef,
};

pub fn completion(set: &RetirementSet) -> Result<Option<ARef<Fence>>> {
    #[cfg(negative)]
    return set.completion();
    #[cfg(not(negative))]
    match set.prepared()? {
        Some(prepared) => prepared.completion(),
        None => Ok(None),
    }
}
