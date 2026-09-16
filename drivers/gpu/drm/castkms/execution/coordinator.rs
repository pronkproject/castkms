// SPDX-License-Identifier: GPL-2.0-only

//! Device-wide exclusion for complete-cohort scene acceptance.

mod installation;
pub(crate) use installation::Update;

use super::validation::{Contract, Epoch, SceneView, Validation};
use crate::scene::Configuration;
use kernel::{
    prelude::*,
    sync::{Arc, Mutex, MutexGuard},
};

struct Pending {
    token: u64,
    owner: Arc<()>,
    configuration: Option<Configuration>,
    target: Contract,
    epoch: Epoch,
    gated: bool,
}

struct Output {
    validation: Validation,
    capability_generation: u64,
    pending: Option<Pending>,
}

/// One coordinator observation, not a reservation or source-read permission.
pub(crate) struct Snapshot {
    pub(crate) epoch: u64,
    pub(crate) active: Contract,
    pub(crate) generation: u64,
    pub(crate) pending: Option<(u64, bool)>,
}

struct State {
    outputs: KVec<Output>,
    count: usize,
    closed: bool,
    next_token: u64,
}

/// A cancellation owner containing no device, framebuffer or native DRM reference.
/// Drop outside the coordinator lock. The metadata slot does not retain this owner.
#[must_use = "dropping the reservation cancels its transition"]
pub(crate) struct Reservation {
    coordinator: Arc<Coordinator>,
    output: usize,
    token: u64,
}

impl Reservation {
    pub(crate) fn token(&self) -> u64 {
        self.token
    }

    pub(crate) fn check(&self) -> Result {
        let guard = self.coordinator.lock();
        guard.pending(self.output, self.token).map(|_| ())
    }

    /// The caller holds current authority, modeset, publication and startup exclusion.
    /// Its compatibility check observes the latest installed-and-published scene.
    /// Publication must perform every fallible step before making execution visible.
    pub(crate) fn activate<R>(
        &self,
        configuration: Option<&Configuration>,
        generation: u64,
        check: impl FnMut(&Contract) -> Result,
        publish: impl FnOnce() -> Result<R>,
    ) -> Result<R> {
        let (result, retired, pending) = {
            let mut guard = self.coordinator.lock();
            let pending = guard.pending(self.output, self.token)?;
            if !pending.gated {
                return Err(EAGAIN);
            }
            if pending.configuration.as_ref() != configuration {
                return Err(ESTALE);
            }
            let slot = guard.0.outputs.get_mut(self.output).ok_or(EINVAL)?;
            if generation <= slot.capability_generation {
                return Err(ESTALE);
            }
            let activation = slot.validation.prepare_activation(self.token, check)?;
            let result = publish()?;
            let retired = activation.commit();
            slot.capability_generation = generation;
            (result, retired, slot.pending.take())
        };
        drop(retired);
        drop(pending);
        Ok(result)
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let retired = {
            let mut guard = self.coordinator.lock();
            guard.cancel(self.output, self.token)
        };
        drop(retired);
    }
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
                        outputs.push(Output {
                            validation: Validation::new(Contract::Host),
                            capability_generation: 1,
                            pending: None,
                        }, GFP_KERNEL)?;
                    }
                    outputs
                },
                count,
                closed: false,
                next_token: 1,
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

    /// Called only inside authorized renderer control. Registration does not restrict
    /// animation; the final tagged native installation will establish that restriction.
    pub(crate) fn reserve(
        self: &Arc<Self>,
        output: usize,
        owner: Arc<()>,
        configuration: Configuration,
        target: Contract,
    ) -> Result<Reservation> {
        let mut guard = self.lock();
        if guard.0.closed {
            return Err(ENODEV);
        }
        let token = guard.0.next_token.checked_add(1).ok_or(EOVERFLOW)?;
        let slot = guard.0.outputs.get_mut(output).ok_or(EINVAL)?;
        if slot.pending.is_some() {
            return Err(EBUSY);
        }
        slot.pending = Some(Pending {
            token,
            owner,
            configuration: Some(configuration),
            target,
            epoch: slot.validation.epoch(),
            gated: false,
        });
        guard.0.next_token = token;
        Ok(Reservation {
            coordinator: self.clone(),
            output,
            token,
        })
    }

    /// Retire reservations issued by one permission owner. Call while excluding
    /// that owner's authorization callbacks, before reporting revocation complete.
    pub(crate) fn revoke_owner(&self, owner: &Arc<()>) {
        self.invalidate_where(|pending| Arc::ptr_eq(&pending.owner, owner));
    }

    /// Invalidate transitions across an authority boundary without closing the device.
    /// The caller excludes registration by the departing authority.
    pub(crate) fn invalidate_all(&self) {
        self.invalidate_where(|_| true);
    }

    fn invalidate_where(&self, matches: impl Fn(&Pending) -> bool) {
        let mut retired: [_; crate::device::MAX_OUTPUTS as usize] =
            core::array::from_fn(|_| (None, None));
        {
            let mut guard = self.lock();
            for (index, slot) in guard.0.outputs.iter_mut().enumerate() {
                if let Some(pending) = slot.pending.as_ref().filter(|pending| matches(pending)) {
                    let token = pending.token;
                    retired[index] = (slot.pending.take(), slot.validation.cancel(token));
                }
            }
        }
        drop(retired);
    }

    /// Exercise gate enforcement without exposing an unauthenticated production setter.
    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(crate) fn gate_for_test(&self, output: usize, proposal: u64, target: Contract) -> Result {
        let mut guard = self.lock();
        if guard.0.closed {
            return Err(ENODEV);
        }
        let validation = &mut guard.0.outputs.get_mut(output).ok_or(EINVAL)?.validation;
        let epoch = validation.epoch();
        validation
            .prepare(epoch, proposal, target, SceneView::Disabled)?
            .commit();
        Ok(())
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(crate) fn cancel_for_test(&self, output: usize, proposal: u64) -> Result {
        let retired = {
            let mut guard = self.lock();
            if guard.0.closed {
                return Err(ENODEV);
            }
            guard
                .0
                .outputs
                .get_mut(output)
                .ok_or(EINVAL)?
                .validation
                .cancel(proposal)
        };
        drop(retired);
        Ok(())
    }
}

impl Guard<'_> {
    pub(crate) fn snapshot(&self, output: usize) -> Result<Snapshot> {
        if self.0.closed {
            return Err(ENODEV);
        }
        let slot = self.0.outputs.get(output).ok_or(EINVAL)?;
        let (epoch, active) = slot.validation.describe();
        Ok(Snapshot {
            epoch,
            active,
            generation: slot.capability_generation,
            pending: slot
                .pending
                .as_ref()
                .map(|pending| (pending.token, pending.gated)),
        })
    }

    fn pending(&self, output: usize, token: u64) -> Result<&Pending> {
        if self.0.closed {
            return Err(ENODEV);
        }
        let slot = self.0.outputs.get(output).ok_or(EINVAL)?;
        slot.pending
            .as_ref()
            .filter(|pending| pending.token == token)
            .ok_or(ESTALE)
    }

    fn cancel(&mut self, output: usize, token: u64) -> (Option<Pending>, Option<Contract>) {
        let Some(slot) = self.0.outputs.get_mut(output) else {
            return (None, None);
        };
        if !slot
            .pending
            .as_ref()
            .is_some_and(|pending| pending.token == token)
        {
            return (None, None);
        }
        (slot.pending.take(), slot.validation.cancel(token))
    }

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
        self.0
            .outputs
            .get(output)
            .ok_or(EINVAL)?
            .validation
            .check(scene)
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
#[kunit_tests(rust_castkms_validation_coordinator)]
mod tests {
    use super::*;
    use kernel::sync::Arc;

    fn configuration() -> Result<Configuration> {
        Configuration::new(1, [640, 480], 60000, 0)
    }

    #[test]
    fn reservations_are_unique_across_outputs_and_cancellation() -> Result {
        let coordinator = Arc::pin_init(Coordinator::new(2), GFP_KERNEL)?;
        let owner = Arc::new((), GFP_KERNEL)?;
        let first = coordinator.reserve(0, owner.clone(), configuration()?, Contract::Host)?;
        let second = coordinator.reserve(1, owner.clone(), configuration()?, Contract::Host)?;
        assert_ne!(first.token(), second.token());
        assert!(matches!(
            coordinator.reserve(0, owner.clone(), configuration()?, Contract::Host),
            Err(EBUSY)
        ));
        assert!(matches!(
            coordinator.lock().pending(1, first.token()),
            Err(ESTALE)
        ));
        let token = first.token();
        drop(first);
        let replacement = coordinator.reserve(0, owner, configuration()?, Contract::Host)?;
        assert!(replacement.token() > token);
        {
            let mut guard = coordinator.lock();
            let retired = guard.cancel(0, token);
            assert!(retired.0.is_none());
        }
        replacement.check()?;
        second.check()?;
        coordinator.close();
        assert_eq!(replacement.check(), Err(ENODEV));
        assert_eq!(second.check(), Err(ENODEV));
        Ok(())
    }

    #[test]
    fn revocation_retires_only_matching_permission_owners() -> Result {
        let coordinator = Arc::pin_init(Coordinator::new(3), GFP_KERNEL)?;
        let owner = Arc::new((), GFP_KERNEL)?;
        let other = Arc::new((), GFP_KERNEL)?;
        let first = coordinator.reserve(0, owner.clone(), configuration()?, Contract::Host)?;
        let second = coordinator.reserve(1, owner.clone(), configuration()?, Contract::Host)?;
        let third = coordinator.reserve(2, other, configuration()?, Contract::Host)?;
        coordinator.gate_for_test(0, first.token(), Contract::Host)?;
        coordinator.revoke_owner(&owner);
        assert_eq!(first.check(), Err(ESTALE));
        assert_eq!(second.check(), Err(ESTALE));
        third.check()?;
        let replacement = coordinator.reserve(0, owner, configuration()?, Contract::Host)?;
        coordinator.gate_for_test(0, replacement.token(), Contract::Host)?;
        drop(first);
        replacement.check()?;
        Ok(())
    }

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
