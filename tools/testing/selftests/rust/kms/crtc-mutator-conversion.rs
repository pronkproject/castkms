// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0271\]
#![no_std]

use kernel::drm::kms::{crtc::*, KmsDriver};

pub fn matching<'a, D: KmsDriver<Crtc = S::Crtc>, S: DriverCrtcState>(
    state: CrtcStateMutator<'a, OpaqueCrtcState<D>>,
) -> CrtcStateMutator<'a, CrtcState<S>>
where
    S::Crtc: DriverCrtc<Driver = D>,
{
    CrtcStateMutator::from_opaque(state)
}

#[cfg(negative)]
pub fn mismatched<'a, D: KmsDriver, S: DriverCrtcState>(
    state: CrtcStateMutator<'a, OpaqueCrtcState<D>>,
) -> CrtcStateMutator<'a, CrtcState<S>> {
    CrtcStateMutator::from_opaque(state)
}
