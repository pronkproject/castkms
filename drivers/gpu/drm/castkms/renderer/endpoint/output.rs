// SPDX-License-Identifier: GPL-2.0-only

//! Recipient output claims, independent of compositor source reads.

use super::{Endpoint, State};
use crate::{
    capture::provider::{delegated_destination::Image, delegated_request::Claim},
    renderer::job::Completion,
};
use kernel::prelude::*;

enum Slot {
    Ready,
    Publishing { id: u64 },
    Claimed { id: u64, claim: Claim },
}

pub(super) struct Stream {
    next_id: u64,
    last_released: Option<u64>,
    slot: Slot,
}

impl Stream {
    pub(super) fn new() -> Self {
        Self { next_id: 1, last_released: None, slot: Slot::Ready }
    }

    pub(super) fn idle(&self) -> bool {
        matches!(self.slot, Slot::Ready)
    }
}

impl Endpoint {
    pub(crate) fn output_readable(&self) -> Result<bool> {
        self.refresh_generation()?;
        let (broker, image) = {
            let state = self.state.lock();
            let State::Ready { publication, pool, output, .. } = &*state else {
                // Empty, configuration and publishing endpoints are idle, not terminal.
                return if matches!(&*state, State::Closed) {
                    Err(EKEYREVOKED)
                } else {
                    Ok(false)
                };
            };
            if !matches!(output.slot, Slot::Ready) { return Ok(false); }
            (publication.control().worker()?.outputs().clone(), pool.first_completed())
        };
        Ok(image.is_some_and(|image| broker.can_claim(&image)))
    }

    pub(crate) fn begin_output(&self, image_id: u64) -> Result<Pending<'_>> {
        self.refresh_generation()?;
        let mut state = self.state.lock();
        let State::Ready { publication, pool, output, .. } = &mut *state else {
            return Err(if matches!(&*state, State::Closed) { EKEYREVOKED } else { ENODATA });
        };
        if !matches!(output.slot, Slot::Ready) {
            return Err(EBUSY);
        }
        let image = pool.completed(image_id)?;
        publication.control().check_completed(&image)?;
        let broker = publication.control().worker()?.outputs().clone();
        let id = output.next_id;
        output.next_id = id.checked_add(1).ok_or(EOVERFLOW)?;
        output.slot = Slot::Publishing { id };
        drop(state);
        let mut pending = Pending { endpoint: self, id, image_id, claim: None };
        pending.claim = Some(broker.try_claim(&image).ok_or(ENODATA)?.output.claim);
        Ok(pending)
    }

    pub(crate) fn release_output(&self, id: u64, completion: Completion) -> Result {
        if id == 0 {
            return Err(EINVAL);
        }
        let claim = {
            let mut state = self.state.lock();
            let State::Ready { output, .. } = &mut *state else {
                return Err(if matches!(&*state, State::Closed) { EKEYREVOKED } else { ENODATA });
            };
            if output.last_released == Some(id) {
                return Ok(());
            }
            match core::mem::replace(&mut output.slot, Slot::Ready) {
                Slot::Claimed { id: current, claim } if current == id => {
                    output.last_released = Some(id);
                    claim
                }
                other => {
                    output.slot = other;
                    return Err(ENOENT);
                }
            }
        };
        claim.release(completion);
        self.changed().notify_all();
        Ok(())
    }
}

#[must_use = "unpublished output claims must be published or dropped"]
pub(crate) struct Pending<'a> {
    endpoint: &'a Endpoint,
    id: u64,
    image_id: u64,
    claim: Option<Claim>,
}

impl Pending<'_> {
    pub(crate) fn id(&self) -> u64 { self.id }
    pub(crate) fn image_id(&self) -> u64 { self.image_id }
    pub(crate) fn destination(&self) -> Result<&Image> {
        Ok(self.claim.as_ref().ok_or(EINVAL)?.destination())
    }

    pub(crate) fn publish(mut self, publish: impl FnOnce()) -> Result {
        let claim = self.claim.take().ok_or(EINVAL)?;
        let mut state = self.endpoint.state.lock();
        let State::Ready { output, .. } = &mut *state else {
            drop(state);
            claim.release(Completion::WithoutAccess);
            return Err(EKEYREVOKED);
        };
        if !matches!(output.slot, Slot::Publishing { id } if id == self.id) {
            drop(state);
            claim.release(Completion::WithoutAccess);
            return Err(ECANCELED);
        }
        publish();
        output.slot = Slot::Claimed { id: self.id, claim };
        Ok(())
    }
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        let claim = self.claim.take();
        let mut state = self.endpoint.state.lock();
        if let State::Ready { output, .. } = &mut *state {
            if matches!(output.slot, Slot::Publishing { id } if id == self.id) {
                output.slot = Slot::Ready;
            }
        }
        drop(state);
        if let Some(claim) = claim {
            claim.release(Completion::WithoutAccess);
        }
        self.endpoint.changed().notify_all();
    }
}
