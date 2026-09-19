// SPDX-License-Identifier: GPL-2.0-only

//! Client-local names for exact host or delegated capture configurations.

use super::{
    client_queue::Queue,
    destination::Image,
    provider::{Capture, Delegated, Description},
};
use crate::capture::output_layout::Layout;
use crate::scene::Configuration;
use kernel::{drm::capture::RequestedLayout, prelude::*};

#[derive(Clone)]
enum Kind {
    Host(Description),
    Delegated(Delegated),
}

#[derive(Clone)]
pub(crate) struct Offered {
    kind: Kind,
    configuration: Configuration,
    dimensions: [u32; 2],
    layout: Layout,
}

impl Offered {
    pub(crate) fn configuration(&self) -> &Configuration {
        &self.configuration
    }
    pub(crate) fn dimensions(&self) -> [u32; 2] {
        self.dimensions
    }
    pub(crate) fn format(&self) -> u32 {
        self.layout.format()
    }
    pub(crate) fn modifier(&self) -> u64 {
        self.layout.modifier()
    }
    pub(crate) fn max_requests(&self) -> u32 {
        match &self.kind {
            Kind::Host(description) => description.max_requests(),
            Kind::Delegated(_) => crate::capture::request_budget::CAPACITY_LIMIT,
        }
    }

    fn open(&self, capacity: u32) -> Result<Queue> {
        match &self.kind {
            Kind::Host(description) => Ok(Queue::Host(
                crate::capture::host_queue::Queue::from_description(description, capacity)?,
            )),
            Kind::Delegated(scope) => Queue::delegated(scope, self.layout, capacity),
        }
    }

    fn retain(&self, image: &mut Image) -> Result {
        match &self.kind {
            Kind::Host(description) => description.capture().retain_destination_storage(image),
            Kind::Delegated(scope) => image.retain_delegated(scope),
        }
    }

    fn same_stream(&self, other: &Self) -> bool {
        if self.configuration != other.configuration || self.layout != other.layout {
            return false;
        }
        match (&self.kind, &other.kind) {
            (Kind::Host(_), Kind::Host(_)) => true,
            (Kind::Delegated(first), Kind::Delegated(second)) => first.same_stream(second),
            _ => false,
        }
    }
}

pub(crate) struct Offer {
    id: u64,
    description: Offered,
}
impl Offer {
    pub(crate) fn id(&self) -> u64 {
        self.id
    }
    pub(crate) fn description(&self) -> &Offered {
        &self.description
    }
}

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

    fn current(&self, requested: RequestedLayout) -> Result<Offered> {
        let layout = if requested.format() == 0 {
            Layout::HOST
        } else {
            Layout::new(requested.format(), requested.modifier())?
        };
        match self.capture.describe_delegated() {
            Ok(scope) => Ok(Offered {
                configuration: scope.configuration().clone(),
                dimensions: scope.dimensions(),
                layout,
                kind: Kind::Delegated(scope),
            }),
            Err(EOPNOTSUPP) => {
                if layout != Layout::HOST {
                    return Err(EOPNOTSUPP);
                }
                let description = self.capture.describe_stream()?;
                let (width, height) = description.layout().dimensions();
                Ok(Offered {
                    configuration: description.configuration().clone(),
                    dimensions: [width, height],
                    layout,
                    kind: Kind::Host(description),
                })
            }
            Err(error) => Err(error),
        }
    }

    pub(super) fn check_capture(&self) -> Result {
        self.current(RequestedLayout::default()).map(|_| ())
    }

    pub(super) fn retain_destination_storage(&self, image: &mut Image) -> Result {
        self.current(RequestedLayout::default())?.retain(image)
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(crate) fn describe(&mut self) -> Result<&Offer> {
        self.describe_layout(RequestedLayout::default())
    }

    pub(crate) fn describe_layout(&mut self, requested: RequestedLayout) -> Result<&Offer> {
        let description = self.current(requested)?;
        let unchanged = self
            .offer
            .as_ref()
            .is_some_and(|offer| offer.description.same_stream(&description));
        if !unchanged {
            let id = self
                .offer
                .as_ref()
                .map_or(Ok(1), |offer| offer.id.checked_add(1).ok_or(EOVERFLOW))?;
            self.offer = Some(Offer { id, description });
        }
        self.offer.as_ref().ok_or(EIO)
    }

    pub(crate) fn open(&self, id: u64, capacity: u32) -> Result<Queue> {
        if id == 0 {
            return Err(EINVAL);
        }
        self.offer
            .as_ref()
            .filter(|offer| offer.id == id)
            .ok_or(ESTALE)?
            .description
            .open(capacity)
    }
}
