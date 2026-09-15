// SPDX-License-Identifier: GPL-2.0-only

//! Submitted candidate work on independent storage, without live-source access.

use crate::scene::ContentSerial;
use kernel::{
    dma_fence::{Fence, Status as FenceStatus},
    prelude::*,
    sync::{
        aref::ARef,
        Mutex, //
    }, //
};

/// Input identity for one private startup operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Source {
    /// Renderer-owned pixels carrying no display content identity.
    Private,
    /// An independent HOST snapshot retaining its historical content identity.
    Snapshot(Option<ContentSerial>),
}

struct Submission {
    source: Source,
    completion: Option<ARef<Fence>>,
}

enum State {
    Idle,
    Publishing,
    Submitted(Submission),
}

/// One candidate's single native probe submission.
///
/// Native completion is retained independently of the submitting task. A successful fence
/// proves only that the submitted operation ended successfully; Source records whether its
/// pixels may represent historical display content.
#[pin_data]
pub(super) struct Probe {
    #[pin]
    state: Mutex<State>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Probe {
    pub(super) fn new() -> impl PinInit<Self> {
        pin_init!(Self {
            state <- kernel::new_mutex!(State::Idle),
        })
    }

    /// Publish one submission, returning the slot when validation fails.
    pub(super) fn submit_then(
        &self,
        source: Source,
        completion: Option<ARef<Fence>>,
        after_reserve: impl FnOnce() -> Result,
    ) -> Result {
        {
            let mut state = self.state.lock();
            match *state {
                State::Idle => *state = State::Publishing,
                _ => return Err(EALREADY),
            }
        }
        let mut pending = Pending {
            probe: self,
            submission: Some(Submission { source, completion }),
        };
        after_reserve()?;
        pending.publish()
    }

    /// Return whether submitted work completed, preserving native failure status.
    pub(super) fn result(&self) -> Result<bool> {
        let state = self.state.lock();
        let submission = match &*state {
            State::Submitted(submission) => submission,
            State::Idle | State::Publishing => return Err(ENODATA),
        };
        match &submission.completion {
            None => Ok(true),
            Some(fence) => match fence.status() {
                FenceStatus::Pending => Ok(false),
                FenceStatus::Complete(result) => result.map(|()| true),
            },
        }
    }

    /// Observe the source identity only after successful native completion.
    pub(super) fn completed_source(&self) -> Result<Source> {
        if !self.result()? {
            return Err(EAGAIN);
        }
        let state = self.state.lock();
        match &*state {
            State::Submitted(submission) => Ok(submission.source),
            State::Idle | State::Publishing => Err(ENODATA),
        }
    }
}

#[must_use = "dropping an unpublished probe submission returns its slot"]
struct Pending<'a> {
    probe: &'a Probe,
    submission: Option<Submission>,
}

impl Pending<'_> {
    fn publish(&mut self) -> Result {
        let submission = self.submission.take().ok_or(EINVAL)?;
        let mut state = self.probe.state.lock();
        if !matches!(*state, State::Publishing) {
            drop(state);
            drop(submission);
            return Err(ECANCELED);
        }
        *state = State::Submitted(submission);
        Ok(())
    }
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        let mut state = self.probe.state.lock();
        if matches!(*state, State::Publishing) {
            *state = State::Idle;
        }
        drop(state);
        drop(self.submission.take());
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
mod tests;
