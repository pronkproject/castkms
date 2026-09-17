// SPDX-License-Identifier: GPL-2.0-only

//! Per-device execution metadata and serialized renderer changes.

use super::{
    Description, //
    Prepared,
    Profile,
};
use crate::Driver;
use kernel::{
    drm::{
        kms::LockedState,
    },
    prelude::*,
    sync::{
        Arc,
        Mutex,
        MutexGuard, //
    }, //
};

/// Brief exclusion against a change away from HOST execution.
///
/// Retain only through source-claim admission. Pixel access and native completion
/// must not depend on this guard, and dropping it performs no renderer work.
pub(crate) struct HostAdmission<'a> {
    _state: MutexGuard<'a, State>,
}

/// Coherent execution and capability metadata, with no retained authority.
pub(crate) struct CapabilitySnapshot {
    pub(crate) execution: Description,
    pub(crate) validation: super::coordinator::Snapshot,
    pub(crate) pending: Option<super::proposal::DescriptionSnapshot>,
}

struct State {
    description: Description,
    slot: Slot,
    next_proposal: u64,
    pending: Option<super::proposal::Entry>,
}

enum Slot {
    Ready,
    Closed,
}

/// Registration ownership closes this device-retaining property before final DRM teardown.
#[pin_data]
pub(crate) struct Publication {
    origin: Arc<()>,
    #[pin]
    state: Mutex<State>,
}

impl Publication {
    pub(crate) fn new() -> impl PinInit<Self, Error> {
        try_pin_init!(Self {
            origin: Arc::new((), GFP_KERNEL)?,
            state <- kernel::new_mutex!(State {
                description: super::initial(),
                slot: Slot::Ready,
                next_proposal: 1,
                pending: None,
            }),
        })
    }

    /// Observe metadata only; retaining it preserves neither authority nor an active renderer.
    pub(crate) fn describe(&self) -> Description {
        self.state.lock().description
    }

    /// Historical pending metadata only; observation grants no renderer authority.
    #[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
    pub(crate) fn pending_profile(&self) -> Option<super::proposal::DescriptionSnapshot> {
        self.state
            .lock()
            .pending
            .as_ref()
            .filter(|entry| entry.reservation.check().is_ok())
            .map(|entry| entry.description.clone())
    }

    /// Publication precedes validation locking, matching activation and registration.
    pub(crate) fn capabilities(
        &self,
        coordinator: &super::coordinator::Coordinator,
        output: usize,
    ) -> Result<CapabilitySnapshot> {
        let state = self.state.lock();
        if matches!(state.slot, Slot::Closed) {
            return Err(ENODEV);
        }
        let validation = coordinator.lock().snapshot(output)?;
        let pending = state
            .pending
            .as_ref()
            .filter(|entry| {
                validation
                    .pending
                    .is_some_and(|(token, _)| token == entry.description.transition)
            })
            .map(|entry| entry.description.clone());
        Ok(CapabilitySnapshot {
            execution: state.description,
            validation,
            pending,
        })
    }

    /// Install one bounded proposal under the caller's display/authority/startup locks.
    /// No scene compatibility or activation is implied by successful registration.
    pub(crate) fn propose(
        self: &Arc<Self>,
        expected: Description,
        worker: &Arc<()>,
        profile: super::validation::Contract,
        reserve: impl FnOnce() -> Result<super::coordinator::Reservation>,
    ) -> Result<super::proposal::Registration> {
        let mut state = self.state.lock();
        match state.slot {
            Slot::Ready => (),
            Slot::Closed => return Err(ENODEV),
        }
        if state.description != expected {
            return Err(ESTALE);
        }
        if state.pending.is_some() {
            return Err(EBUSY);
        }
        let generation = super::proposal::next_generation(state.next_proposal)?;
        let reservation = reserve()?;
        let description = super::proposal::DescriptionSnapshot {
            generation,
            transition: reservation.token(),
            expected,
            profile,
        };
        state.pending = Some(super::proposal::Entry {
            description: description.clone(),
            worker: worker.clone(),
            reservation,
        });
        state.next_proposal = generation;
        Ok(super::proposal::Registration {
            publication: self.clone(),
            description,
        })
    }

    pub(super) fn check_proposal(&self, generation: u64) -> Result {
        let state = self.state.lock();
        if matches!(state.slot, Slot::Closed) {
            return Err(ENODEV);
        }
        match &state.pending {
            Some(entry)
                if entry.description.generation == generation
                    && entry.description.expected == state.description =>
            {
                entry.reservation.check()
            }
            _ => Err(ESTALE),
        }
    }

    pub(super) fn cancel_proposal(&self, generation: u64) {
        let retired = {
            let mut state = self.state.lock();
            if state
                .pending
                .as_ref()
                .is_some_and(|entry| entry.description.generation == generation)
            {
                state.pending.take()
            } else {
                None
            }
        };
        drop(retired);
    }

    /// Cancel only the named worker's proposal, without touching active execution.
    pub(crate) fn cancel_worker_proposal(&self, worker: &Arc<()>) {
        let retired = {
            let mut state = self.state.lock();
            if state
                .pending
                .as_ref()
                .is_some_and(|entry| Arc::ptr_eq(&entry.worker, worker))
            {
                state.pending.take()
            } else {
                None
            }
        };
        drop(retired);
    }

    /// A newly reserved worker supersedes metadata from an invalidated reservation.
    /// The caller must hold its current, exclusive startup reservation.
    pub(crate) fn retire_other_worker_proposal(&self, worker: &Arc<()>) {
        let retired = {
            let mut state = self.state.lock();
            if state
                .pending
                .as_ref()
                .is_some_and(|entry| !Arc::ptr_eq(&entry.worker, worker))
            {
                state.pending.take()
            } else {
                None
            }
        };
        drop(retired);
    }

    /// Exclude execution publication while admitting one HOST source read.
    pub(crate) fn admit_host(&self) -> Result<HostAdmission<'_>> {
        let state = self.state.lock();
        if state.description.profile != Profile::HostV1 {
            return Err(EOPNOTSUPP);
        }
        Ok(HostAdmission { _state: state })
    }

    /// Check whether a HOST-only operation is currently meaningful.
    pub(crate) fn check_host(&self) -> Result {
        drop(self.admit_host()?);
        Ok(())
    }

    /// Allocate a new description before entering display or renderer control locks.
    ///
    /// Preparation alone changes no capability. Publication rechecks origin, generation,
    /// device and shutdown state; the caller separately authorizes the renderer handoff.
    pub(crate) fn prepare(&self, profile: Profile) -> Result<Prepared> {
        let expected = {
            let state = self.state.lock();
            match state.slot {
                Slot::Ready => state.description,
                Slot::Closed => return Err(ENODEV),
            }
        };
        Prepared::new(self.origin.clone(), expected, profile)
    }

    /// Publish execution and its gated input contract under the same installation lock.
    pub(crate) fn publish_proposal(
        &self,
        _locked: &LockedState<'_, Driver>,
        prepared: &mut Prepared,
        generation: u64,
        configuration: Option<&crate::scene::Configuration>,
        check: impl FnMut(&super::validation::Contract) -> Result,
    ) -> Result {
        if !Arc::ptr_eq(&self.origin, &prepared.origin) {
            return Err(EINVAL);
        }
        let change = prepared.pending.ok_or(EALREADY)?;
        let retired = {
            let mut state = self.state.lock();
            if state.description != change.expected {
                return Err(ESTALE);
            }
            let State {
                description,
                slot,
                pending,
                ..
            } = &mut *state;
            match slot {
                Slot::Ready => (),
                Slot::Closed => return Err(ENODEV),
            }
            let entry = pending
                .as_ref()
                .filter(|entry| entry.description.generation == generation)
                .ok_or(ESTALE)?;
            entry
                .reservation
                .activate(configuration, generation, check, || {
                    *description = change.next;
                    prepared.pending = None;
                    Ok(())
                })?;
            pending.take()
        };
        drop(retired);
        Ok(())
    }

    /// Close the internal control handle and release any pending proposal outside its mutex.
    pub(crate) fn close(&self) {
        let pending = {
            let mut state = self.state.lock();
            state.slot = Slot::Closed;
            state.pending.take()
        };
        drop(pending);
    }
}
