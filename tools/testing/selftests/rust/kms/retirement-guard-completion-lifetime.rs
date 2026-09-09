// SPDX-License-Identifier: GPL-2.0 OR MIT
// error-pattern: cannot return value referencing function parameter `guard`
#![no_std]

use kernel::{dma_fence::Fence, drm::preparation::RetirementGuard};

#[cfg(negative)]
pub fn escape(guard: RetirementGuard) -> Option<&'static Fence> {
    guard.completion()
}

#[cfg(not(negative))]
pub fn borrow(guard: &RetirementGuard) -> Option<&Fence> {
    guard.completion()
}
