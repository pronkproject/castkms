// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0596\]
#![no_std]

use kernel::drm::kms::{atomic::*, connector::*, crtc::*, KmsDriver};

pub fn callback<C: DriverCrtc>(
    commit: CrtcAtomicCommit<'_, C>,
    connector: &Connector<<C::Driver as KmsDriver>::Connector>,
) {
    core::hint::black_box(commit.take_state().get_new_connector_state(connector));
}

#[cfg(negative)]
pub fn mutate<D: KmsDriver>(reader: &AtomicStateReader<D>, connector: &Connector<D::Connector>) {
    let state = reader.get_new_connector_state(connector).unwrap();
    core::hint::black_box(&mut **state);
}
