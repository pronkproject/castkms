// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0271\]
#![no_std]

use kernel::drm::kms::crtc::*;

pub fn reciprocal<C: DriverCrtc>() {
    fn state_of<C: DriverCrtc, S: DriverCrtcState<Crtc = C>>() {}
    state_of::<C, C::State>();
}

#[cfg(negative)]
mod invalid {
    use super::*;
    use core::marker::PhantomData;
    use kernel::{drm::Device, prelude::*};

    pub struct Other<C>(C);

    #[vtable]
    impl<C: DriverCrtc> DriverCrtc for Other<C> {
        type Args = ();
        type Driver = C::Driver;
        type State = C::State;
        type VblankImpl = PhantomData<Self>;

        fn new(_: &Device<Self::Driver>, _: &()) -> impl PinInit<Self, Error> {
            Err::<Self, Error>(EINVAL)
        }
    }
}
