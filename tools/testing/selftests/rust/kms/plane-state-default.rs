// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0277\]
#![no_std]

use kernel::drm::kms::plane::*;

// Driver-private defaults remain available; only DRM constructs the wrapper
// that promises an initialized parent plane.
pub fn private_default<S: DriverPlaneState>() -> S {
    S::default()
}

pub fn parent<S: DriverPlaneState>(state: &PlaneState<S>) -> &Plane<S::Plane> {
    state.plane()
}

#[cfg(negative)]
pub fn parentless<S: DriverPlaneState>() -> PlaneState<S> {
    Default::default()
}
