// SPDX-License-Identifier: GPL-2.0-only

//! Independently authorized private-image-to-recipient writes and native retirement.

use super::delegated_destination::{Image, Use};
use crate::{
    renderer::{job::Completion, render_job::Rendered},
    scene::ContentSerial,
};
use kernel::{
    dma_fence::{
        retirement::{Retire, Retirement},
        Fence, Status as FenceStatus,
    },
    prelude::*,
    sync::{aref::ARef, poll::PollCondVar, Arc, Mutex},
    time::{Instant, Monotonic},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Pending,
    Complete(Result),
    /// Native access is unresolved. Storage is quarantined, not reusable.
    Lost,
}

#[derive(Clone, Copy)]
enum Phase {
    Queued,
    Claimed,
    Submitted,
    Complete(Result),
    Lost,
}

struct State {
    phase: Phase,
    cancelled: bool,
    content: Option<ContentSerial>,
    completed_at: Option<Instant<Monotonic>>,
    usage: Option<Arc<Use>>,
    completion: Option<ARef<Fence>>,
}

/// One destination use, initially without private pixels or compositor source access.
#[pin_data]
pub(crate) struct Request {
    destination: Arc<Image>,
    changed: Option<Arc<PollCondVar>>,
    _charge: Option<Arc<crate::capture::request_budget::Charge>>,
    #[pin]
    state: Mutex<State>,
}

impl Image {
    pub(super) fn request_accounted(
        self: &Arc<Self>,
        reuse: Option<ARef<Fence>>,
        charge: Option<Arc<crate::capture::request_budget::Charge>>,
        changed: Option<Arc<PollCondVar>>,
    ) -> Result<Arc<Request>> {
        let request = Arc::pin_init(
            pin_init!(Request {
                destination: self.clone(),
                changed,
                _charge: charge,
                state <- kernel::new_mutex!(State { phase: Phase::Queued, cancelled: false, content: None, completed_at: None, usage: None, completion: None }),
            }),
            GFP_KERNEL,
        )?;
        let usage = self.reserve_notified(reuse, request.changed.clone())?;
        request.state.lock().usage = Some(usage);
        Ok(request)
    }
}

impl Request {
    fn notify(&self) {
        if let Some(changed) = &self.changed {
            changed.notify_all();
        }
    }

    /// Stop unclaimed work immediately; claimed work retains storage until native retirement.
    pub(crate) fn cancel(&self) {
        let retired = {
            let mut state = self.state.lock();
            match state.phase {
                Phase::Queued => {
                    if let Some(usage) = &state.usage {
                        usage.retire();
                    }
                    state.phase = Phase::Complete(Err(ECANCELED));
                    state.usage.take()
                }
                Phase::Claimed | Phase::Submitted => {
                    state.cancelled = true;
                    None
                }
                Phase::Complete(_) | Phase::Lost => None,
            }
        };
        drop(retired);
        self.notify();
    }

    fn fail_queued(&self, error: Error) {
        let retired = {
            let mut state = self.state.lock();
            if matches!(state.phase, Phase::Queued) {
                if let Some(usage) = &state.usage {
                    usage.retire();
                }
                state.phase = Phase::Complete(Err(error));
                state.usage.take()
            } else {
                None
            }
        };
        drop(retired);
        self.notify();
    }

    /// Reconcile queued authority and reuse failure without requiring a private image.
    /// Submitted work remains pending until actual cleanup.
    pub(crate) fn status(&self) -> Status {
        if matches!(self.state.lock().phase, Phase::Queued) {
            if let Err(error) = self.destination.scope().with_current(|_| Ok(())) {
                self.fail_queued(error);
            }
        }
        let usage = {
            let state = self.state.lock();
            if matches!(state.phase, Phase::Queued) {
                state.usage.clone()
            } else {
                None
            }
        };
        if let Some(usage) = usage {
            if let Err(error) = usage.ready() {
                self.fail_queued(error);
            }
        }
        match self.state.lock().phase {
            Phase::Queued | Phase::Claimed | Phase::Submitted => Status::Pending,
            Phase::Complete(result) => Status::Complete(result),
            Phase::Lost => Status::Lost,
        }
    }

    /// Reconcile worker loss without retaining its active ownership or claiming an image.
    pub(crate) fn status_for_worker(&self) -> Status {
        if matches!(self.state.lock().phase, Phase::Queued) {
            if let Err(error) = self.destination.scope().check_worker() {
                self.fail_queued(error);
            }
        }
        self.status()
    }

    pub(crate) fn ready_for(&self, image: &Rendered) -> bool {
        let result = (|| {
            let usage = {
                let state = self.state.lock();
                if !matches!(state.phase, Phase::Queued) { return Ok(false); }
                state.usage.as_ref().ok_or(EIO)?.clone()
            };
            self.destination.scope().with_worker(|current| image.content().check_bound(current))?;
            let source = match image.content().status() {
                FenceStatus::Pending => false,
                FenceStatus::Complete(result) => { result?; true }
            };
            Ok(source && usage.ready()?)
        })();
        match result {
            Ok(ready) => ready,
            Err(error) => { self.fail_queued(error); false }
        }
    }

    pub(crate) fn content_serial(&self) -> Option<ContentSerial> {
        let state = self.state.lock();
        if matches!(state.phase, Phase::Complete(Ok(()))) {
            state.content
        } else {
            None
        }
    }

    pub(crate) fn completed_at(&self) -> Option<Instant<Monotonic>> {
        let state = self.state.lock();
        if matches!(state.phase, Phase::Complete(Ok(()))) {
            state.completed_at
        } else {
            None
        }
    }

    /// Claim one bounded E-to-D stage only after source production and destination reuse
    /// succeeded. `Ok(None)` is pending; every error terminates an unclaimed request.
    /// Native queues and mappings must isolate this output stage from source-reading work.
    pub(crate) fn try_claim(
        self: &Arc<Self>,
        image: &Arc<Rendered>,
    ) -> Result<Option<Claim>> {
        let result = self.prepare_claim(image);
        if let Err(error) = result.as_ref() {
            self.fail_queued(*error);
        }
        result
    }

    fn prepare_claim(
        self: &Arc<Self>,
        image: &Arc<Rendered>,
    ) -> Result<Option<Claim>> {
        let usage = {
            let state = self.state.lock();
            match state.phase {
                Phase::Queued => state.usage.as_ref().ok_or(EIO)?.clone(),
                Phase::Complete(Err(error)) => return Err(error),
                Phase::Lost => return Err(EIO),
                _ => return Err(EALREADY),
            }
        };
        // Authority loss is terminal even while a downstream reuse fence is pending.
        self.destination.scope().with_current(|_| Ok(()))?;
        self.destination.scope().with_worker(|_| Ok(()))?;
        let source_ready = match image.content().status() {
            FenceStatus::Pending => false,
            FenceStatus::Complete(result) => {
                result?;
                true
            }
        };
        let destination_ready = usage.ready()?;
        if !source_ready || !destination_ready {
            return Ok(None);
        }
        let completed_at = image.content().completed_at()?;
        let report = Arc::pin_init(
            pin_init!(Report { completion <- kernel::new_mutex!(None) }),
            GFP_KERNEL,
        )?;
        let retirement = Retirement::new(Hold {
            request: self.clone(),
            image: image.clone(),
            report: report.clone(),
        })?;
        self.destination
            .scope()
            .with_image(image, |current| {
                if current.uses_reservation(usage.image().buffer().reservation())? {
                    return Err(EINVAL);
                }
                let mut state = self.state.lock();
                if !matches!(state.phase, Phase::Queued) {
                    return Err(ECANCELED);
                }
                state.content = image.content().content_serial();
                state.completed_at = Some(completed_at);
                state.phase = Phase::Claimed;
                Ok(())
            })?;
        Ok(Some(Claim {
            request: self.clone(),
            image: image.clone(),
            report,
            retirement: Some(retirement),
        }))
    }

    /// Called only after access ended. Returned storage owners are dropped outside policy locks.
    fn complete(&self, result: Result) -> Option<Arc<Use>> {
        let mut state = self.state.lock();
        if !matches!(state.phase, Phase::Submitted) {
            return None;
        }
        if let Some(usage) = &state.usage {
            usage.retire();
        }
        state.phase = Phase::Complete(if state.cancelled {
            Err(ECANCELED)
        } else {
            result
        });
        let usage = state.usage.take();
        drop(state);
        self.notify();
        usage
    }
}

#[pin_data]
struct Report {
    #[pin]
    completion: Mutex<Option<Completion>>,
}

struct Hold {
    request: Arc<Request>,
    image: Arc<Rendered>,
    report: Arc<Report>,
}

// SAFETY: The local module owns the callback and all CastKMS destructors it invokes.
#[vtable]
unsafe impl Retire for Hold {
    fn retire(self) {
        let completion = self.report.completion.lock().take();
        let Some(completion) = completion else {
            return;
        };
        let native = match completion {
            Completion::Cpu => Ok(()),
            Completion::WithoutAccess => Err(ECANCELED),
            Completion::Submitted(fence) => match fence.status() {
                FenceStatus::Complete(result) => result,
                FenceStatus::Pending => Err(EIO),
            },
        };
        let scoped = self.request.destination.scope().with_image(
            &self.image, |_| Ok(self.request.complete(native)),
        );
        let retired = match scoped {
            Ok(retired) => retired,
            Err(error) => self.request.complete(Err(error)),
        };
        drop(retired);
        // Dropping the retained image releases E independently of result dequeue or D reuse.
    }
}

/// Preauthorized bounded output stage. Dropping it without a report is terminal worker loss.
#[must_use = "output claims must report native completion or confirm no access"]
pub(crate) struct Claim {
    request: Arc<Request>,
    image: Arc<Rendered>,
    report: Arc<Report>,
    retirement: Option<Retirement<Hold>>,
}

impl Claim {
    /// Exact recipient storage and checked layout admitted for this output stage.
    /// Retaining a DMA-BUF reference grants no write beyond the claimed stage.
    pub(crate) fn destination(&self) -> &Image {
        &self.request.destination
    }

    /// Recheck recipient permission, worker readiness and accepted image before
    /// publishing the destination descriptor. A claimed but unpublished stage
    /// carries no userspace access and may still be released without access.
    pub(crate) fn publish_if_current(&self, publish: impl FnOnce()) -> Result {
        self.request.destination.scope().with_image(&self.image, |_| {
            publish();
            Ok(())
        })
    }

    /// Report only materialized native work covering E reads and D writes. CPU completion
    /// includes cache maintenance. No-access promises that neither allocation was accessed.
    pub(crate) fn release(mut self, completion: Completion) {
        let fence = match &completion {
            Completion::Submitted(fence) => Some(fence.clone()),
            _ => None,
        };
        *self.report.completion.lock() = Some(completion);
        {
            let mut state = self.request.state.lock();
            state.completion = fence.clone();
            state.phase = Phase::Submitted;
        }
        self.request.notify();
        if let Some(retirement) = self.retirement.take() {
            match fence {
                Some(fence) => retirement.submit(&fence),
                None => drop(retirement),
            }
        }
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        if let Some(retirement) = self.retirement.take() {
            self.request.state.lock().phase = Phase::Lost;
            self.request.notify();
            // Unknown access is not completion. Retain E, D, accounting and callback code;
            // native-driver recovery or a future explicit recovery protocol must resolve it.
            core::mem::forget(retirement);
        }
    }
}
