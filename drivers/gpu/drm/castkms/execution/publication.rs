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

struct State {
    description: Description,
    slot: Slot,
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

    /// Release the control handle outside its mutex, preserving the installed native blob.
    pub(crate) fn close(&self) {
        let slot = {
            let mut state = self.state.lock();
            core::mem::replace(&mut state.slot, Slot::Closed)
        };
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
