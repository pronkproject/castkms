// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0277\]
#![no_std]

use core::marker::PhantomData;
use kernel::drm::kms::{crtc::DriverCrtc, vblank::*};

pub fn supported<C, V>()
where
    C: DriverCrtc<VblankImpl = V>,
    V: VblankSupport<Crtc = C>,
{
    fn requires_support<T: VblankDriverCrtc>() {}
    requires_support::<C>();
}

#[cfg(negative)]
pub struct Disabled<C>(PhantomData<fn() -> C>);

#[cfg(negative)]
impl<C> VblankDriverCrtc for Disabled<C> where Self: DriverCrtc<VblankImpl = PhantomData<Self>> {}
