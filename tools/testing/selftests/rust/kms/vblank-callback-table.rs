// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0277\]
#![no_std]

use core::marker::PhantomData;
use kernel::drm::kms::{crtc::DriverCrtc, vblank::*};

pub fn generated<V: VblankSupport>() {
    fn implementation<I: VblankImpl>() {}
    implementation::<V>();
}

pub fn disabled<C: DriverCrtc<VblankImpl = PhantomData<C>>>() {
    fn implementation<I: VblankImpl>() {}
    implementation::<PhantomData<C>>();
}

#[cfg(negative)]
pub struct Replacement<C, V>(PhantomData<fn() -> (C, V)>);

#[cfg(negative)]
impl<C, V> VblankImpl for Replacement<C, V>
where
    C: DriverCrtc<VblankImpl = Self>,
    V: VblankImpl,
{
    type Crtc = C;
    const VBLANK_OPS: VblankOps = V::VBLANK_OPS;
}
