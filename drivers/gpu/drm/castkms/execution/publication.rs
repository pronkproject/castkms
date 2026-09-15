// SPDX-License-Identifier: GPL-2.0-only

//! Per-device execution metadata and explicit lifetime of its connector property.

use super::{
    property,
    Description, //
    Prepared,
    Profile,
};
use crate::{
    display::Connector,
    Driver, //
};
use kernel::{
    drm::{
        device::Registered,
        kms::{
            connector::{
                ReadOnlyBlobProperty,
                UnregisteredConnector, //
            },
            LockedState, //
        },
        Device, //
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
    Empty,
    Attaching,
    Ready(ReadOnlyBlobProperty<Connector>),
    Closed,
}

struct Attachment<'a>(&'a Publication);

impl Drop for Attachment<'_> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock();
        if matches!(state.slot, Slot::Attaching) {
            state.slot = Slot::Empty;
        }
    }
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
                slot: Slot::Empty,
                next_proposal: 1,
                pending: None,
            }),
        })
    }

    /// Attach once during unpublished KMS construction, without outer DRM control locks.
    pub(crate) fn attach(&self, connector: &UnregisteredConnector<Connector>) -> Result {
        let (description, _attachment) = self.reserve_attachment()?;
        let property = property::attach(connector, description)?;
        {
            let mut state = self.state.lock();
            if !matches!(state.slot, Slot::Attaching) {
                return Err(ENODEV);
            }
            state.slot = Slot::Ready(property);
        }
        Ok(())
    }

    fn reserve_attachment(&self) -> Result<(Description, Attachment<'_>)> {
        let description = {
            let mut state = self.state.lock();
            match state.slot {
                Slot::Closed => return Err(ENODEV),
                Slot::Empty => state.slot = Slot::Attaching,
                _ => return Err(EALREADY),
            }
            state.description
        };
        Ok((description, Attachment(self)))
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
            Slot::Ready(_) => (),
            Slot::Closed => return Err(ENODEV),
            _ => return Err(EAGAIN),
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
    #[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
    pub(crate) fn prepare(
        &self,
        device: &Device<Driver, Registered>,
        profile: Profile,
    ) -> Result<Prepared> {
        let expected = {
            let state = self.state.lock();
            match state.slot {
                Slot::Ready(_) => state.description,
                Slot::Closed => return Err(ENODEV),
                _ => return Err(EAGAIN),
            }
        };
        Prepared::new(device, self.origin.clone(), expected, profile)
    }

    /// Publish prepared metadata during authorized control, retaining retired ownership.
    ///
    /// The caller coordinates actual renderer admission and execution eligibility within
    /// the same control interval. This operation alone starts no renderer or source read.
    /// Neither allocation nor native reference release occurs under the publication mutex.
    #[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
    pub(crate) fn publish(
        &self,
        locked: &LockedState<'_, Driver>,
        prepared: &mut Prepared,
    ) -> Result {
        if !Arc::ptr_eq(&self.origin, &prepared.origin) {
            return Err(EINVAL);
        }
        let change = prepared.pending.ok_or(EALREADY)?;
        let mut state = self.state.lock();
        if state.description != change.expected {
            return Err(ESTALE);
        }
        // Probe-only activation cannot bypass a proposed capability contract.
        if state.pending.is_some() {
            return Err(EAGAIN);
        }
        let property = match &mut state.slot {
            Slot::Ready(property) => property,
            Slot::Closed => return Err(ENODEV),
            _ => return Err(EAGAIN),
        };
        property.replace_blob(locked, &mut prepared.blob)?;
        state.description = change.next;
        prepared.pending = None;
        Ok(())
    }

    /// Publish execution and its gated input contract under the same installation lock.
    pub(crate) fn publish_proposal(
        &self,
        locked: &LockedState<'_, Driver>,
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
            let entry = pending
                .as_ref()
                .filter(|entry| entry.description.generation == generation)
                .ok_or(ESTALE)?;
            let property = match slot {
                Slot::Ready(property) => property,
                Slot::Closed => return Err(ENODEV),
                _ => return Err(EAGAIN),
            };
            entry
                .reservation
                .activate(configuration, generation, check, || {
                    property.replace_blob(locked, &mut prepared.blob)?;
                    *description = change.next;
                    prepared.pending = None;
                    Ok(())
                })?;
            pending.take()
        };
        drop(retired);
        Ok(())
    }

    /// Release the control handle outside its mutex, preserving the installed native blob.
    pub(crate) fn close(&self) {
        let (slot, pending) = {
            let mut state = self.state.lock();
            (
                core::mem::replace(&mut state.slot, Slot::Closed),
                state.pending.take(),
            )
        };
        drop(pending);
        if let Slot::Ready(property) = slot {
            drop(property);
        }
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
#[kunit_tests(rust_castkms_execution_attachment)]
mod tests {
    use super::*;
    use kernel::sync::Arc;

    #[test]
    fn abandoned_attachment_returns_the_empty_slot() -> Result {
        let publication = Arc::pin_init(Publication::new(), GFP_KERNEL)?;
        let (_, attachment) = publication.reserve_attachment()?;
        assert_eq!(publication.reserve_attachment().err(), Some(EALREADY));
        drop(attachment);
        let (_, replacement) = publication.reserve_attachment()?;
        drop(replacement);
        let empty = matches!(publication.state.lock().slot, Slot::Empty);
        assert!(empty);
        Ok(())
    }

    #[test]
    fn abandoned_attachment_does_not_reopen_a_closed_slot() -> Result {
        let publication = Arc::pin_init(Publication::new(), GFP_KERNEL)?;
        let (_, attachment) = publication.reserve_attachment()?;
        publication.close();
        drop(attachment);
        assert_eq!(publication.reserve_attachment().err(), Some(ENODEV));
        let closed = matches!(publication.state.lock().slot, Slot::Closed);
        assert!(closed);
        Ok(())
    }
}
