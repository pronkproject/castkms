// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0502\]
#![no_std]

use kernel::drm::kms::{atomic::*, connector::*, crtc::*, KmsDriver};

pub fn read<D: KmsDriver>(state: &mut AtomicStateMutator<D>, crtc: &Crtc<D::Crtc>) {
    core::hint::black_box(state.new_connector_state_for_crtc(crtc));
}

#[cfg(negative)]
pub fn conflicting<D: KmsDriver>(
    state: &mut AtomicStateMutator<D>,
    crtc: &Crtc<D::Crtc>,
    connector: &Connector<D::Connector>,
) {
    let opaque = state.new_connector_state_for_crtc(crtc).unwrap();
    let shared = ConnectorState::<<D::Connector as DriverConnector>::State>::from_opaque(opaque);
    let mut guard = state.get_new_connector_state(connector).unwrap();
    core::hint::black_box(&mut *guard);
    core::hint::black_box(shared);
}
