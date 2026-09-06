// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0277\]
#![no_std]

use kernel::drm::kms::plane::*;

pub fn shared<S: DriverPlaneState>() {
    fn transferable<T: Send + Sync>() {}
    transferable::<S>();
}

#[cfg(negative)]
#[derive(Clone, Default)]
pub struct ThreadBound<P>(kernel::types::NotThreadSafe, core::marker::PhantomData<fn() -> P>);

#[cfg(negative)]
impl<P> DriverPlaneState for ThreadBound<P>
where
    P: DriverPlane<State = Self> + Clone + Default,
{
    type Plane = P;
}
