// SPDX-License-Identifier: GPL-2.0-only

//! Borrow the whole validation cohort until native installation has succeeded.

use super::*;
use crate::execution::validation::Installation;

const OUTPUTS: usize = crate::device::MAX_OUTPUTS as usize;

/// Metadata borrowed only during validation, never retained by the resulting change.
pub(crate) struct Update<'a> {
    pub(crate) scene: SceneView<'a>,
    pub(crate) token: u64,
    pub(crate) previous_configuration: Option<&'a Configuration>,
    pub(crate) configuration: Option<&'a Configuration>,
}

struct Tagged<'a> {
    gate: Installation<'a>,
    pending: &'a mut Pending,
    configuration: Option<Configuration>,
}

enum Change<'a> {
    Tag(Tagged<'a>),
    Cancel(&'a mut Output),
}

/// Plain retired metadata, released after the installation coordinator is unlocked.
pub(crate) struct Retired {
    _entries: [(Option<Pending>, Option<Contract>); OUTPUTS],
}

/// Holds every affected validation owner exclusively. Dropping does not install a gate.
#[must_use = "commit only in the native installation success continuation"]
pub(crate) struct Prepared<'a> {
    changes: [Option<Change<'a>>; OUTPUTS],
}

impl Guard<'_> {
    pub(crate) fn prepare<'g>(
        &'g mut self,
        updates: &[Option<Update<'_>>; OUTPUTS],
    ) -> Result<Prepared<'g>> {
        let mut prepared = Prepared {
            changes: core::array::from_fn(|_| None),
        };
        for (index, update) in updates.iter().enumerate() {
            if let Some(update) = update {
                self.check(index, update.scene)?;
                if self.0.closed && update.token != 0 {
                    return Err(ENODEV);
                }
            }
        }
        for (index, slot) in self.0.outputs.iter_mut().enumerate() {
            let Some(update) = &updates[index] else {
                continue;
            };
            if update.token == 0 {
                if slot
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.configuration.as_ref() != update.configuration)
                {
                    prepared.changes[index] = Some(Change::Cancel(slot));
                }
                continue;
            }
            let pending = slot
                .pending
                .as_mut()
                .filter(|pending| pending.token == update.token)
                .ok_or(ESTALE)?;
            if pending.configuration.as_ref() != update.previous_configuration {
                return Err(ESTALE);
            }
            if pending.gated {
                // A repeated tag is an observation of the same installed gate, not a
                // second mode migration or an opportunity to renew its identity.
                if pending.configuration.as_ref() != update.configuration {
                    return Err(ESTALE);
                }
                continue;
            }
            let gate = slot.validation.prepare(
                pending.epoch,
                pending.token,
                pending.target.clone(),
                update.scene,
            )?;
            prepared.changes[index] = Some(Change::Tag(Tagged {
                gate,
                pending,
                configuration: update.configuration.cloned(),
            }));
        }
        Ok(prepared)
    }
}

impl Prepared<'_> {
    /// No allocation, failure or native object release remains after successful swap.
    pub(crate) fn commit(self) -> Retired {
        let mut entries = core::array::from_fn(|_| (None, None));
        for (index, change) in self.changes.into_iter().enumerate() {
            match change {
                Some(Change::Tag(change)) => {
                    change.gate.commit();
                    change.pending.configuration = change.configuration;
                    change.pending.gated = true;
                }
                Some(Change::Cancel(slot)) => {
                    if let Some(pending) = slot.pending.take() {
                        let contract = slot.validation.cancel(pending.token);
                        entries[index] = (Some(pending), contract);
                    }
                }
                None => (),
            }
        }
        Retired { _entries: entries }
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
#[kunit_tests(rust_castkms_transition_installation)]
mod tests {
    use super::*;

    #[test]
    fn unrelated_configuration_changes_retire_only_after_installation() -> Result {
        for gated in [false, true] {
            let coordinator = Arc::pin_init(Coordinator::new(1), GFP_KERNEL)?;
            let configuration = Configuration::new(1, [640, 480], 60000, 0)?;
            let replacement = Configuration::new(1, [800, 600], 60000, 0)?;
            let reservation = coordinator.reserve(
                0,
                Arc::new((), GFP_KERNEL)?,
                configuration.clone(),
                Contract::Host,
            )?;
            let mut updates = core::array::from_fn(|_| None);
            updates[0] = Some(Update {
                scene: SceneView::Disabled,
                token: reservation.token(),
                previous_configuration: Some(&configuration),
                configuration: Some(&configuration),
            });
            let mut guard = coordinator.lock();
            if gated {
                guard.prepare(&updates)?.commit();
            }
            let epoch = guard.0.outputs[0].validation.epoch();
            let update = updates[0].as_mut().ok_or(EINVAL)?;
            update.token = 0;
            // Compatible animation retains the pending transition.
            guard.prepare(&updates)?.commit();
            assert!(guard.pending(0, reservation.token()).is_ok());
            updates[0].as_mut().ok_or(EINVAL)?.configuration = Some(&replacement);
            drop(guard.prepare(&updates)?);
            assert!(guard.pending(0, reservation.token()).is_ok());
            assert_eq!(guard.0.outputs[0].validation.epoch(), epoch);
            let retired = guard.prepare(&updates)?.commit();
            assert!(matches!(guard.pending(0, reservation.token()), Err(ESTALE)));
            if gated {
                assert_ne!(guard.0.outputs[0].validation.epoch(), epoch);
            }
            drop(guard);
            drop(retired);
            assert_eq!(reservation.check(), Err(ESTALE));
        }
        Ok(())
    }

    #[test]
    fn complete_cohort_changes_only_on_success() -> Result {
        let coordinator = Arc::pin_init(Coordinator::new(8), GFP_KERNEL)?;
        let configuration = Configuration::new(1, [640, 480], 60000, 0)?;
        let owner = Arc::new((), GFP_KERNEL)?;
        let mut reservations = KVec::with_capacity(8, GFP_KERNEL)?;
        for index in 0..8 {
            reservations.push(
                coordinator.reserve(index, owner.clone(), configuration.clone(), Contract::Host)?,
                GFP_KERNEL,
            )?;
        }
        let mut updates = core::array::from_fn(|index| {
            Some(Update {
                scene: SceneView::Disabled,
                token: reservations[index].token(),
                previous_configuration: Some(&configuration),
                configuration: None,
            })
        });
        let mut guard = coordinator.lock();
        updates[7].as_mut().ok_or(EINVAL)?.token = reservations[0].token();
        assert!(matches!(guard.prepare(&updates), Err(ESTALE)));
        updates[7].as_mut().ok_or(EINVAL)?.token = reservations[7].token();
        // TEST_ONLY and failed native installation both drop an uncommitted change.
        drop(guard.prepare(&updates)?);
        for slot in &guard.0.outputs {
            assert!(!slot.pending.as_ref().ok_or(EINVAL)?.gated);
            assert_eq!(
                slot.validation.epoch(),
                slot.pending.as_ref().ok_or(EINVAL)?.epoch
            );
        }
        guard.prepare(&updates)?.commit();
        for slot in &guard.0.outputs {
            assert!(slot.pending.as_ref().ok_or(EINVAL)?.gated);
            assert!(slot.pending.as_ref().ok_or(EINVAL)?.configuration.is_none());
            assert_ne!(
                slot.validation.epoch(),
                slot.pending.as_ref().ok_or(EINVAL)?.epoch
            );
        }
        // Retries do not renew the gate or require an unchanged content serial.
        for update in updates.iter_mut().flatten() {
            update.previous_configuration = None;
        }
        guard.prepare(&updates)?.commit();
        updates[0].as_mut().ok_or(EINVAL)?.configuration = Some(&configuration);
        assert!(matches!(guard.prepare(&updates), Err(ESTALE)));
        Ok(())
    }
}
