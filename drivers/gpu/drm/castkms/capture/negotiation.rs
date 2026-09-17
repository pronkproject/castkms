// SPDX-License-Identifier: GPL-2.0-only

//! Client-local names for exact host or delegated capture configurations.

use super::{client_queue::Queue, destination::Image, provider::{Capture, Delegated, Description}};
use crate::scene::Configuration;
use kernel::{drm::fourcc, prelude::*};

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
}

impl Offered {
    pub(crate) fn configuration(&self) -> &Configuration { &self.configuration }
    pub(crate) fn dimensions(&self) -> [u32; 2] { self.dimensions }
    pub(crate) fn format(&self) -> u32 { fourcc::XRGB8888 }
    pub(crate) fn modifier(&self) -> u64 { fourcc::FORMAT_MOD_LINEAR }
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
            Kind::Delegated(scope) => Queue::delegated(scope, capacity),
        }
    }

    fn retain(&self, image: &mut Image) -> Result {
        match &self.kind {
            Kind::Host(description) => description.capture().retain_destination_storage(image),
            Kind::Delegated(scope) => image.retain_delegated(scope),
        }
    }


    fn same_stream(&self, other: &Self) -> bool {
        if self.configuration != other.configuration { return false; }
        match (&self.kind, &other.kind) {
            (Kind::Host(_), Kind::Host(_)) => true,
            (Kind::Delegated(first), Kind::Delegated(second)) => first.same_stream(second),
            _ => false,
        }
    }
}

pub(crate) struct Offer { id: u64, description: Offered }
impl Offer {
    pub(crate) fn id(&self) -> u64 { self.id }
    pub(crate) fn description(&self) -> &Offered { &self.description }
}

pub(crate) struct Negotiation { capture: Capture, offer: Option<Offer> }

impl Negotiation {
    pub(crate) fn new(capture: Capture) -> Self { Self { capture, offer: None } }

    fn current(&self) -> Result<Offered> {
        match self.capture.describe_delegated() {
            Ok(scope) => Ok(Offered {
                configuration: scope.configuration().clone(),
                dimensions: scope.dimensions(),
                kind: Kind::Delegated(scope),
            }),
            Err(EOPNOTSUPP) => {
                let description = self.capture.describe_stream()?;
                let (width, height) = description.layout().dimensions();
                Ok(Offered {
                    configuration: description.configuration().clone(),
                    dimensions: [width, height],
                    kind: Kind::Host(description),
                })
            }
            Err(error) => Err(error),
        }
    }

    pub(super) fn check_capture(&self) -> Result { self.current().map(|_| ()) }

    pub(super) fn retain_destination_storage(&self, image: &mut Image) -> Result {
        self.current()?.retain(image)
    }

    pub(crate) fn describe(&mut self) -> Result<&Offer> {
        let description = self.current()?;
        let unchanged = self.offer.as_ref().is_some_and(|offer| {
            offer.description.same_stream(&description)
        });
        if !unchanged {
            let id = self.offer.as_ref().map_or(Ok(1), |offer| {
                offer.id.checked_add(1).ok_or(EOVERFLOW)
            })?;
            self.offer = Some(Offer { id, description });
        }
        self.offer.as_ref().ok_or(EIO)
    }

    pub(crate) fn open(&self, id: u64, capacity: u32) -> Result<Queue> {
        if id == 0 { return Err(EINVAL); }
        self.offer.as_ref().filter(|offer| offer.id == id).ok_or(ESTALE)?.description.open(capacity)
    }
}
