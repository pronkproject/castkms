// SPDX-License-Identifier: GPL-2.0-only

//! One native renderer probe, without live-source access.

use kernel::{
    dma_fence::{Fence, Status as FenceStatus},
    prelude::*,
    sync::{
        aref::ARef,
        Mutex, //
    }, //
};

struct Submission {
    completion: Option<ARef<Fence>>,
}

enum SubmissionState {
    Idle,
    Publishing,
    Submitted(Submission),
}

struct State {
    submission: SubmissionState,
}

/// One endpoint declaration's single native probe submission.
///
/// Native completion is retained independently of the submitting task. A successful fence
/// proves only that the submitted operation ended successfully.
#[pin_data]
pub(super) struct Probe {
    #[pin]
    state: Mutex<State>,
}

impl Probe {
    pub(super) fn new() -> impl PinInit<Self> {
        pin_init!(Self {
            state <- kernel::new_mutex!(State {
                submission: SubmissionState::Idle,
            }),
        })
    }

    /// Publish one submission, returning the slot when validation fails.
    pub(super) fn submit_then(
        &self,
        completion: Option<ARef<Fence>>,
        after_reserve: impl FnOnce() -> Result,
    ) -> Result {
        {
            let mut state = self.state.lock();
            if !matches!(state.submission, SubmissionState::Idle) {
                return Err(EALREADY);
            }
            state.submission = SubmissionState::Publishing;
        }
        let mut pending = Pending {
            probe: self,
            submission: Some(Submission { completion }),
        };
        after_reserve()?;
        pending.publish()
    }

    /// Distinguish pending work from terminal native errors without interpreting errno.
    pub(super) fn status(&self) -> Result<FenceStatus> {
        let state = self.state.lock();
        let submission = match &state.submission {
            SubmissionState::Submitted(submission) => submission,
            SubmissionState::Idle | SubmissionState::Publishing => return Err(ENODATA),
        };
        Ok(submission.completion.as_ref().map_or(
            FenceStatus::Complete(Ok(())), |fence| fence.status(),
        ))
    }

    /// Return whether submitted work completed, preserving native failure status.
    pub(super) fn result(&self) -> Result<bool> {
        match self.status()? {
            FenceStatus::Pending => Ok(false),
            FenceStatus::Complete(result) => result.map(|()| true),
        }
    }

    /// Require successful native completion.
    pub(super) fn completed(&self) -> Result {
        if !self.result()? {
            return Err(EAGAIN);
        }
        let state = self.state.lock();
        match &state.submission {
            SubmissionState::Submitted(_) => Ok(()),
            SubmissionState::Idle | SubmissionState::Publishing => Err(ENODATA),
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
        if !matches!(state.submission, SubmissionState::Publishing) {
            drop(state);
            drop(submission);
            return Err(ECANCELED);
        }
        state.submission = SubmissionState::Submitted(submission);
        Ok(())
    }
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        let mut state = self.probe.state.lock();
        if matches!(state.submission, SubmissionState::Publishing) {
            state.submission = SubmissionState::Idle;
        }
        drop(state);
        drop(self.submission.take());
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
mod tests;
