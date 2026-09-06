// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0277\]
#![no_std]

use kernel::drm::kms::crtc::*;

pub fn shared<S: DriverCrtcState>() {
    fn transferable<T: Send + Sync>() {}
    transferable::<S>();
}

#[cfg(negative)]
#[derive(Clone, Default)]
pub struct ThreadBound<C>(kernel::types::NotThreadSafe, core::marker::PhantomData<fn() -> C>);

#[cfg(negative)]
impl<C> DriverCrtcState for ThreadBound<C>
where
    C: DriverCrtc<State = Self> + Clone + Default + Unpin,
{
    type Crtc = C;
}
