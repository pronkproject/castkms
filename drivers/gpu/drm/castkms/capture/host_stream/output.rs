// SPDX-License-Identifier: GPL-2.0-only

//! One retained destination for a private capture attempt, without source access while waiting.

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
    Copying(Frame),
    Finished,
}

/// A single output attempt retaining its exact storage and explicit reuse dependency.
///
/// Waiting for reuse retains a private capture result, never compositor storage. The
/// owner must exclude conflicting access to the destination until completion or drop.
/// Dropping pending output abandons its capture request without canceling shared rendering.
#[must_use = "dropping output abandons the pending delivery"]
pub(crate) struct Output {
    state: State,
    destination: Arc<Image>,
    reuse: Option<ARef<Fence>>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
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

    /// Observe rendering and attempt copying without waiting for unfinished dependencies.
    ///
    /// Authorization, allocation and CPU copying may sleep; call outside DRM, publication,
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
            State::Copying(frame) => frame,
            State::Finished => return Err(EALREADY),
        };
        if frame
            .request()
            .try_copy_to_destination(&self.destination, self.reuse.as_deref())?
        {
            Ok(Some(frame))
        } else {
            self.state = State::Copying(frame);
            Ok(None)
        }
    }
}
