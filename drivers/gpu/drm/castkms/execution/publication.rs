// SPDX-License-Identifier: GPL-2.0-only

//! Per-device execution metadata and explicit lifetime of its connector property.

use super::{
    property,
    Description, //
};
use crate::display::Connector;
use kernel::{
    drm::kms::connector::{
        ReadOnlyBlobProperty,
        UnregisteredConnector, //
    },
    prelude::*,
    sync::Mutex, //
};

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
    #[pin]
    state: Mutex<State>,
}

impl Publication {
    pub(crate) fn new() -> impl PinInit<Self> {
        pin_init!(Self {
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
