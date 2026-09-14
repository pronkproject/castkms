// SPDX-License-Identifier: GPL-2.0-only

//! One retained destination for a private capture attempt, without source access while waiting.

mod delivery;

use super::Pending;
use crate::capture::{
    destination::Image,
    provider::Frame, //
};
use kernel::{
    dma_fence::Fence,
    prelude::*,
    sync::{
        aref::ARef,
        Arc, //
    }, //
};

enum State {
    Capturing(Pending),
    Captured(Frame),
    Delivering(delivery::Handle),
    Cancelled,
    Finished,
}

/// A single output attempt retaining its exact storage and explicit reuse dependency.
///
/// Waiting for reuse retains a private capture result, never compositor storage. The
/// owner must exclude conflicting access until actual completion. Drop abandons observation
/// and requests cancellation, but does not acknowledge completion of detached destination
/// access. No compositor source is retained by that access.
#[must_use = "dropping output abandons the pending delivery"]
pub(crate) struct Output {
    state: State,
    destination: Arc<Image>,
    reuse: Option<ARef<Fence>>,
}

impl Output {
    pub(crate) fn new(
        pending: Pending,
        destination: Arc<Image>,
        reuse: Option<ARef<Fence>>,
    ) -> Self {
        Self {
            state: State::Capturing(pending),
            destination,
            reuse,
        }
    }

    /// Cancel an unfinished output without waiting for rendering or destination reuse.
    ///
    /// A private image waiting for reuse is discarded without touching the destination.
    /// Detached access observes cancellation after exporter calls return; its eventual result
    /// remains pending until access ends. Cancellation does not finish shared source reads.
    /// Completed output and repeated cancellation return EALREADY.
    pub(crate) fn cancel(&mut self) -> Result {
        match &self.state {
            State::Capturing(pending) => pending.cancel()?,
            State::Captured(_) => (),
            State::Delivering(delivery) => {
                return delivery.cancel();
            }
            State::Cancelled | State::Finished => return Err(EALREADY),
        }
        self.state = State::Cancelled;
        Ok(())
    }

    /// Whether detached access still prevents acknowledgment of storage reuse.
    pub(crate) fn is_delivering(&self) -> bool {
        matches!(&self.state, State::Delivering(delivery) if delivery.is_running())
    }

    /// Observe rendering and dispatch copying without entering destination exporters.
    ///
    /// Authorization, allocation and private copying may sleep; call outside DRM, publication,
    /// reservation and worker-lifecycle locks. `None` preserves the exact pending attempt.
    /// A frame is returned only after its destination write and cache maintenance finish.
    /// Every error consumes the attempt, even EAGAIN; subsequent calls return EALREADY.
    /// Returned metadata belongs to that private image, not a later displayed scene.
    pub(crate) fn try_complete_frame(&mut self) -> Result<Option<Frame>> {
        let frame = match core::mem::replace(&mut self.state, State::Finished) {
            State::Capturing(mut pending) => match pending.try_complete_frame()? {
                Some(frame) => frame,
                None => {
                    self.state = State::Capturing(pending);
                    return Ok(None);
                }
            },
            State::Captured(frame) => frame,
            State::Delivering(delivery) => match delivery.take_result() {
                None => {
                    self.state = State::Delivering(delivery);
                    return Ok(None);
                }
                Some((frame, result)) => {
                    if result? {
                        return Ok(Some(frame));
                    }
                    frame
                }
            },
            State::Cancelled => return Err(ECANCELED),
            State::Finished => return Err(EALREADY),
        };
        if frame
            .request()
            .destination_ready(&self.destination, self.reuse.as_deref())?
        {
            self.state = State::Delivering(delivery::Handle::new(
                frame,
                self.destination.clone(),
                self.reuse.clone(),
            )?);
            Ok(None)
        } else {
            self.state = State::Captured(frame);
            Ok(None)
        }
    }
}
