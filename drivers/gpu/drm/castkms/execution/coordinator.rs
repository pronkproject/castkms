// SPDX-License-Identifier: GPL-2.0-only

//! Device-wide exclusion for complete-cohort scene acceptance.

use super::validation::{Contract, SceneView, Validation};
use kernel::{
    prelude::*,
    sync::{Mutex, MutexGuard},
};

struct State {
    outputs: KVec<Validation>,
    count: usize,
    closed: bool,
}

/// Modeset locks precede this lock; native preparation locks follow it.
///
/// One lock covers all outputs so installation can validate a complete transaction
/// before swapping any state. Do not wait for producers, enter renderer control,
/// acquire modeset locks or release final DRM references while holding it.
#[pin_data]
pub(crate) struct Coordinator {
    #[pin]
    state: Mutex<State>,
}

pub(crate) struct Guard<'a>(MutexGuard<'a, State>);

impl Coordinator {
    pub(crate) fn new(count: usize) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {
            state <- kernel::new_mutex!(State {
                outputs: {
                    if count == 0 || count > crate::device::MAX_OUTPUTS as usize {
                        return Err(EINVAL);
                    }
                    let mut outputs = KVec::with_capacity(count, GFP_KERNEL)?;
                    for _ in 0..count {
                        outputs.push(Validation::new(Contract::Host), GFP_KERNEL)?;
                    }
                    outputs
                },
                count,
                closed: false,
            }),
        })
    }

    /// Retain across the final check and native installation, not just the check.
    pub(crate) fn lock(&self) -> Guard<'_> {
        Guard(self.state.lock())
    }

    pub(crate) fn close(&self) {
        let retired = {
            let mut state = self.state.lock();
            state.closed = true;
            core::mem::replace(&mut state.outputs, KVec::new())
        };
        drop(retired);
    }


}

impl Guard<'_> {
    pub(crate) fn check(&self, output: usize, scene: SceneView<'_>) -> Result {
        if output >= self.0.count {
            return Err(EINVAL);
        }
        if self.0.closed {
            // DRM still needs its final disable transaction to shut down vblank and
            // release accepted state after the driver's admission owners have closed.
            return match scene {
                SceneView::Disabled => Ok(()),
                SceneView::Enabled { .. } => Err(ENODEV),
            };
        }
        self.0.outputs.get(output).ok_or(EINVAL)?.check(scene)
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
#[kunit_tests(rust_castkms_validation_coordinator)]
mod tests {
    use super::*;
    use kernel::sync::Arc;

    #[test]
    fn output_bounds_match_device_construction() -> Result {
        assert!(matches!(
            Arc::pin_init(Coordinator::new(0), GFP_KERNEL),
            Err(EINVAL)
        ));
        assert!(matches!(
            Arc::pin_init(Coordinator::new(9), GFP_KERNEL),
            Err(EINVAL)
        ));
        let coordinator = Arc::pin_init(Coordinator::new(8), GFP_KERNEL)?;
        let guard = coordinator.lock();
        for index in 0..8 {
            guard.check(index, SceneView::Disabled)?;
        }
        assert_eq!(guard.check(8, SceneView::Disabled), Err(EINVAL));
        Ok(())
    }

    #[test]
    fn close_preserves_only_the_final_disable_path() -> Result {
        let coordinator = Arc::pin_init(Coordinator::new(1), GFP_KERNEL)?;
        let scene = crate::scene::Scene::blank(None);
        for _ in 0..2 {
            coordinator.close();
            let guard = coordinator.lock();
            guard.check(0, SceneView::Disabled)?;
            assert_eq!(guard.check(1, SceneView::Disabled), Err(EINVAL));
            assert_eq!(
                guard.check(
                    0,
                    SceneView::Enabled {
                        scene: &scene,
                        output: [640, 480],
                    }
                ),
                Err(ENODEV)
            );
        }
        Ok(())
    }
}
