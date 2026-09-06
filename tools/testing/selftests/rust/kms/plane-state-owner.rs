// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0271\]
#![no_std]

use kernel::drm::kms::plane::*;

pub fn reciprocal<P: DriverPlane, S: DriverPlaneState>() {
    fn state_of<P: DriverPlane, S: DriverPlaneState<Plane = P>>() {}
    fn plane_of<S: DriverPlaneState, P: DriverPlane<State = S>>() {}
    state_of::<P, P::State>();
    plane_of::<S, S::Plane>();
}

#[cfg(negative)]
#[derive(Clone, Default)]
pub struct Other<S>(S);

#[cfg(negative)]
impl<S: DriverPlaneState> DriverPlaneState for Other<S> {
    type Plane = S::Plane;

    fn new(_: &Plane<Self::Plane>) -> kernel::error::Result<Self> {
        Err(kernel::error::code::EINVAL)
    }

    fn duplicate(&self) -> kernel::error::Result<Self> {
        Err(kernel::error::code::EINVAL)
    }
}
