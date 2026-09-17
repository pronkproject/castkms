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
pub(crate) enum Source {
    /// Renderer-owned pixels carrying no display content identity.
    Private,
    /// An independent HOST snapshot retaining its historical content identity.
    Snapshot(Option<ContentSerial>),
}

struct Submission {
    source: Source,
    completion: Option<ARef<Fence>>,
}

enum SubmissionState {
    Idle,
    Publishing,
    Submitted(Submission),
}

struct Snapshot {
    content: Option<ContentSerial>,
}

struct State {
    snapshot: Option<Snapshot>,
    submission: SubmissionState,
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

impl Probe {
    pub(super) fn new() -> impl PinInit<Self> {
        pin_init!(Self {
            state <- kernel::new_mutex!(State {
                snapshot: None,
                submission: SubmissionState::Idle,
            }),
        })
    }

    /// Publish one startup snapshot and retain its kernel-derived content identity.
    ///
    /// The callback runs under the probe lock and must not reenter candidate operations.
    /// It should only perform the infallible descriptor installation guarded by publication.
    pub(super) fn publish_snapshot<R>(
        &self,
        content: Option<ContentSerial>,
        publish: impl FnOnce() -> R,
    ) -> Result<R> {
        let mut state = self.state.lock();
        if state.snapshot.is_some() {
            return Err(EALREADY);
        }
        let result = publish();
        state.snapshot = Some(Snapshot { content });
        Ok(result)
    }

    /// Publish one submission, returning the slot when validation fails.
    pub(super) fn submit_then(
        &self,
        source: Source,
        completion: Option<ARef<Fence>>,
        after_reserve: impl FnOnce() -> Result,
    ) -> Result {
        self.submit_source_then(|_| Ok(source), completion, after_reserve)
    }

    fn submit_source_then(
        &self,
        source: impl FnOnce(&State) -> Result<Source>,
        completion: Option<ARef<Fence>>,
        after_reserve: impl FnOnce() -> Result,
    ) -> Result {
        let source = {
            let mut state = self.state.lock();
            if !matches!(state.submission, SubmissionState::Idle) {
                return Err(EALREADY);
            }
            let source = source(&state)?;
            state.submission = SubmissionState::Publishing;
            source
        };
        let mut pending = Pending {
            probe: self,
            submission: Some(Submission { source, completion }),
        };
        after_reserve()?;
        pending.publish()
    }

    /// Publish work over the startup snapshot delivered for this candidate.
    pub(super) fn submit_snapshot_then(
        &self,
        completion: Option<ARef<Fence>>,
        after_reserve: impl FnOnce() -> Result,
    ) -> Result {
        self.submit_source_then(
            |state| {
                let content = state.snapshot.as_ref().ok_or(ENODATA)?.content;
                Ok(Source::Snapshot(content))
            },
            completion,
            after_reserve,
        )
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

    /// Observe the source identity only after successful native completion.
    pub(super) fn completed_source(&self) -> Result<Source> {
        if !self.result()? {
            return Err(EAGAIN);
        }
        let state = self.state.lock();
        match &state.submission {
            SubmissionState::Submitted(submission) => Ok(submission.source),
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
