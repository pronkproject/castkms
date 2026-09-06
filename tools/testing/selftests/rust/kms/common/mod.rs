// SPDX-License-Identifier: GPL-2.0
#![allow(dead_code)]

use kernel::drm::kms::{atomic::*, KmsDriver};

pub fn conventional<'a, D: KmsDriver>(
    mut tail: AtomicCommitTail<'a, D>,
    modesets: ModesetsReadyToken<'a, D>,
    planes: PlaneUpdatesReadyToken<'a, D>,
) -> CommittedAtomicState<'a, D> {
    let disabled = tail.commit_modeset_disables(modesets);
    let planes = tail.commit_planes(planes, PlaneCommitFlags::default());
    let enabled = tail.commit_modeset_enables(disabled);
    tail.commit_hw_done(enabled, planes)
}

pub fn runtime_pm<'a, D: KmsDriver>(
    mut tail: AtomicCommitTail<'a, D>,
    modesets: ModesetsReadyToken<'a, D>,
    planes: PlaneUpdatesReadyToken<'a, D>,
) -> CommittedAtomicState<'a, D> {
    let disabled = tail.commit_modeset_disables(modesets);
    let enabled = tail.commit_modeset_enables(disabled);
    let planes = tail.commit_planes(planes, PlaneCommitFlags::default());
    tail.commit_hw_done(enabled, planes)
}

pub fn callback_signature<D: KmsDriver>() {
    let _: for<'a> fn(
        AtomicCommitTail<'a, D>,
        ModesetsReadyToken<'a, D>,
        PlaneUpdatesReadyToken<'a, D>,
    ) -> CommittedAtomicState<'a, D> = D::atomic_commit_tail;
}
