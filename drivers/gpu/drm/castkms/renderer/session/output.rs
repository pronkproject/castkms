// SPDX-License-Identifier: GPL-2.0-only

//! Transactional publication of independently authorized private-image output jobs.

use super::{Session, Slot as SessionSlot};
use crate::{
    capture::provider::{delegated_destination::Image, delegated_request::Claim},
    renderer::job::Completion,
};
use kernel::prelude::*;

pub(super) enum Slot {
    Ready,
    Publishing { id: u64 },
    Claimed { id: u64, claim: Claim },
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Session {
    /// Claim at most one ready recipient for the named completed private image.
    /// Source-stage admission is independent: E-to-D never acquires another A read.
    pub(crate) fn begin_output(&self, image_id: u64) -> Result<Pending<'_>> {
        let mut state = self.state.lock();
        if state.closed {
            return Err(EKEYREVOKED);
        }
        let super::State { slot, images, .. } = &mut *state;
        let SessionSlot::Renderer {
            candidate,
            active,
            route,
            output,
            next_output_id,
            ..
        } = slot
        else {
            return Err(EOPNOTSUPP);
        };
        if !matches!(output, Slot::Ready) {
            return Err(EBUSY);
        }
        candidate.with_observed_control(&active.observation(), |_| Ok(()))?;
        let image = images.as_ref().ok_or(ESHUTDOWN)?.completed(image_id)?;
        let id = *next_output_id;
        let next = id.checked_add(1).ok_or(EOVERFLOW)?;
        let broker = route.broker();
        *next_output_id = next;
        *output = Slot::Publishing { id };
        drop(state);
        let mut pending = Pending {
            session: self,
            id,
            image_id,
            claim: None,
        };
        // Destination reuse and recipient admission never hold source-session exclusion.
        let job = broker.try_claim(&image).ok_or(ENODATA)?;
        pending.claim = Some(job.output.claim);
        Ok(pending)
    }

    /// Retain native E reads and D writes independently of the endpoint's next job.
    /// Cleanup accepts the latest repeated release even after capture revocation.
    pub(crate) fn release_output(&self, id: u64, completion: Completion) -> Result {
        if id == 0 {
            return Err(EINVAL);
        }
        let claim = {
            let mut state = self.state.lock();
            let SessionSlot::Renderer {
                output,
                last_released_output,
                ..
            } = &mut state.slot
            else {
                return Err(ENOENT);
            };
            if *last_released_output == Some(id) {
                return Ok(());
            }
            match core::mem::replace(output, Slot::Ready) {
                Slot::Claimed { id: current, claim } if current == id => {
                    *last_released_output = Some(id);
                    claim
                }
                other => {
                    *output = other;
                    return Err(ENOENT);
                }
            }
        };
        claim.release(completion);
        self.access.device().changed.notify_all();
        Ok(())
    }
}

/// An unpublished claim returns with no access on every failed adapter operation.
#[must_use = "dropping an unpublished output releases it without native access"]
pub(crate) struct Pending<'a> {
    session: &'a Session,
    id: u64,
    image_id: u64,
    claim: Option<Claim>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Pending<'_> {
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    pub(crate) fn image_id(&self) -> u64 {
        self.image_id
    }

    pub(crate) fn destination(&self) -> Result<&Image> {
        Ok(self.claim.as_ref().ok_or(EINVAL)?.destination())
    }

    /// Install only after all fallible descriptor construction and copyout has finished.
    pub(crate) fn publish(mut self, publish: impl FnOnce()) -> Result {
        let claim = self.claim.take().ok_or(EINVAL)?;
        let mut state = self.session.state.lock();
        let failure = if state.closed {
            Some(EKEYREVOKED)
        } else {
            match &state.slot {
                SessionSlot::Renderer {
                    output: Slot::Publishing { id },
                    ..
                } if *id == self.id => None,
                _ => Some(ECANCELED),
            }
        };
        if let Some(error) = failure {
            drop(state);
            claim.release(Completion::WithoutAccess);
            return Err(error);
        }
        let SessionSlot::Renderer { output, .. } = &mut state.slot else {
            drop(state);
            claim.release(Completion::WithoutAccess);
            return Err(EIO);
        };
        publish();
        *output = Slot::Claimed { id: self.id, claim };
        Ok(())
    }
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        let claim = self.claim.take();
        let mut state = self.session.state.lock();
        let mut restored = false;
        if let SessionSlot::Renderer { output, .. } = &mut state.slot {
            if matches!(output, Slot::Publishing { id } if *id == self.id) {
                *output = Slot::Ready;
                restored = true;
            }
        }
        drop(state);
        if let Some(claim) = claim {
            claim.release(Completion::WithoutAccess);
        }
        if restored {
            self.session.access.device().changed.notify_all();
        }
    }
}
