// SPDX-License-Identifier: GPL-2.0-only

//! Client-local names for negotiated configurations, independent of descriptor transport.

use super::{
    host_queue::Queue,
    provider::{
        Capture,
        Description, //
    }, //
};
use kernel::prelude::*;

/// One client's latest offered configuration; the number is not capture authority.
pub(crate) struct Offer {
    id: u64,
    description: Description,
}

impl Offer {
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    pub(crate) fn description(&self) -> &Description {
        &self.description
    }
}

/// Retain at most one description without reserving stream storage or reading pixels.
///
/// Names increase within this client lifetime and are never reused. Querying unchanged
/// configuration preserves its name; observing a replacement invalidates the previous
/// offer. Existing streams keep their own lifetimes and authorization checks. The owner
/// must serialize these operations outside DRM, publication and reservation locks.
pub(crate) struct Negotiation {
    capture: Capture,
    offer: Option<Offer>,
}

impl Negotiation {
    pub(crate) fn new(capture: Capture) -> Self {
        Self {
            capture,
            offer: None,
        }
    }

    /// Recheck capture permission without changing this client's offered configuration.
    pub(super) fn check_capture(&self) -> Result {
        self.capture.describe_stream().map(|_| ())
    }

    pub(super) fn retain_destination_storage(&self, image: &mut super::destination::Image) -> Result {
        self.capture.retain_destination_storage(image)
    }

    /// Observe current permission before returning even an unchanged offer.
    ///
    /// Failure leaves the previous description intact but grants no right to use it.
    /// Repeated queries after failed publication return the same name while the display
    /// configuration remains current. Exhaustion requires a new client lifetime.
    pub(crate) fn describe(&mut self) -> Result<&Offer> {
        let description = self.capture.describe_stream()?;
        let unchanged = self
            .offer
            .as_ref()
            .is_some_and(|offer| offer.description.configuration() == description.configuration());
        if !unchanged {
            let id = next_id(self.offer.as_ref().map_or(0, |offer| offer.id))?;
            self.offer = Some(Offer { id, description });
        }
        self.offer.as_ref().ok_or(EIO)
    }

    /// Open only the named offer, rechecking its grant and configuration at admission.
    ///
    /// No implicit description query substitutes a later configuration. Failure does not
    /// consume the offer; valid retry and additional streams share the same description.
    pub(crate) fn open(&self, id: u64, capacity: u32) -> Result<Queue> {
        if id == 0 {
            return Err(EINVAL);
        }
        let offer = self
            .offer
            .as_ref()
            .filter(|offer| offer.id == id)
            .ok_or(ESTALE)?;
        Queue::from_description(&offer.description, capacity)
    }
}

fn next_id(previous: u64) -> Result<u64> {
    previous.checked_add(1).ok_or(EOVERFLOW)
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
#[kunit_tests(rust_castkms_capture_offer_ids)]
mod tests {
    use super::*;

    #[test]
    fn exhaustion_never_reuses_an_offer_name() -> Result {
        if next_id(0) != Ok(1)
            || next_id(u64::MAX - 1) != Ok(u64::MAX)
            || next_id(u64::MAX) != Err(EOVERFLOW)
        {
            return Err(EINVAL);
        }
        Ok(())
    }
}
