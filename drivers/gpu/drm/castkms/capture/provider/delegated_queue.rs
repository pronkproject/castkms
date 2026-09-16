// SPDX-License-Identifier: GPL-2.0-only

//! Bounded recipient demand for independently claimed private-image output.

use super::{
    delegated::Delegated,
    delegated_destination::Image,
    delegated_request::{
        Claim,
        Request,
        Status, //
    }, //
};
use crate::{
    capture::{
        request_budget::Charge,
        requests, //
    },
    renderer::{
        candidate::Candidate,
        render_job::Rendered, //
    },
    renderer_startup::{
        Active,
        Observation, //
    },
    scene::ContentSerial, //
};
use kernel::{
    dma_fence::Fence,
    prelude::*,
    sync::{
        aref::ARef,
        poll::PollCondVar,
        Arc, //
    }, //
};

struct Pending {
    use_id: u64,
    request: Arc<Request>,
}

/// A retained terminal outcome. A native fence is cleanup evidence, not pixel validity.
#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
pub(crate) struct Completion {
    pub(crate) use_id: u64,
    pub(crate) result: Result,
    pub(crate) content: Option<ContentSerial>,
    pub(crate) native: Option<ARef<Fence>>,
}

/// One bounded output claim, named independently of its private source image.
#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
pub(crate) struct Job {
    pub(crate) use_id: u64,
    pub(crate) claim: Claim,
}

/// Queue operations require sleepable context outside display and reservation locks.
/// The queue observes its worker; only the worker's owner keeps that incarnation active.
pub(crate) struct Queue {
    scope: Delegated,
    renderer: Arc<Candidate>,
    active: Observation,
    changed: Arc<PollCondVar>,
    records: requests::Queue<Pending, Arc<Request>>,
    charge: Arc<Charge>,
    closing: bool,
    lost: bool,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Delegated {
    pub(crate) fn create_queue(
        &self,
        renderer: &Arc<Candidate>,
        active: &Active,
        capacity: u32,
    ) -> Result<Queue> {
        self.create_queue_observed(renderer, &active.observation(), capacity)
    }

    /// Retain worker identity without borrowing or extending endpoint ownership.
    pub(crate) fn create_queue_observed(
        &self,
        renderer: &Arc<Candidate>,
        active: &Observation,
        capacity: u32,
    ) -> Result<Queue> {
        self.with_renderer(renderer, active, |_| Ok(()))?;
        let charge = self.request_budget().reserve(capacity)?;
        let records = requests::Queue::new(charge.capacity())?;
        self.with_renderer(renderer, active, |_| Ok(()))?;
        Ok(Queue {
            scope: self.clone(),
            renderer: renderer.clone(),
            active: active.clone(),
            changed: self.changed(),
            records,
            charge,
            closing: false,
            lost: false,
        })
    }
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Queue {
    /// Register poll waiters before reconciling authority, dependencies and terminal results.
    /// Shared wakeups may belong to another queue or output and convey no pixel authority.
    pub(crate) fn changed(&self) -> &Arc<PollCondVar> {
        &self.changed
    }

    /// Accept demand only for storage registered under this exact recipient interval.
    /// No source claim, private image, or permission to access pixels is acquired here.
    pub(crate) fn queue_to(
        &mut self,
        use_id: u64,
        destination: &Arc<Image>,
        reuse: Option<ARef<Fence>>,
    ) -> Result {
        if self.closing {
            return Err(ESHUTDOWN);
        }
        self.scope.check_same(destination.scope())?;
        self.scope
            .with_renderer(&self.renderer, &self.active, |_| Ok(()))?;
        self.records.queue(use_id, || {
            Ok(Pending {
                use_id,
                request: destination.request_accounted(
                    use_id,
                    reuse,
                    Some(self.charge.clone()),
                    Some(self.changed.clone()),
                )?,
            })
        })?;
        self.changed.notify_all();
        Ok(())
    }

    /// Select one ready destination without waiting on earlier requests or native work.
    /// Every failed admission becomes that request's terminal result, including EAGAIN.
    pub(crate) fn try_claim(&mut self, image: &Arc<Rendered>) -> Option<Job> {
        if self.closing {
            self.advance();
            return None;
        }
        let mut claimed = None;
        self.records.for_each_pending(|pending| {
            if claimed.is_some() {
                return;
            }
            if let Ok(Some(claim)) =
                pending
                    .request
                    .try_claim_observed(&self.renderer, &self.active, image)
            {
                claimed = Some(Job {
                    use_id: pending.use_id,
                    claim,
                });
            }
        });
        self.advance();
        claimed
    }

    pub(crate) fn cancel(&mut self, use_id: u64) -> Result {
        self.records.cancel(use_id, |pending| {
            if pending.request.status_for(&self.renderer, &self.active) != Status::Pending {
                return Err(EALREADY);
            }
            pending.request.cancel();
            Ok(())
        })
    }

    /// Reconcile authority and native completion without reading or retaining a source.
    pub(crate) fn advance(&mut self) -> usize {
        self.records.advance(|pending| {
            match pending.request.status_for(&self.renderer, &self.active) {
                Status::Pending => Ok(None),
                Status::Complete(_) => Ok(Some(pending.request.clone())),
                Status::Lost => {
                    self.lost = true;
                    Ok(Some(pending.request.clone()))
                }
            }
        })
    }

    pub(crate) fn has_results(&self) -> bool {
        self.records.has_results()
    }

    pub(crate) fn has_pending(&self) -> bool {
        self.records.has_pending()
    }

    /// Copy/publish before acknowledging. Failure preserves the exact request record.
    /// Successful native completion is rechecked against current publication authority.
    pub(crate) fn dequeue<R>(
        &mut self,
        publish: impl FnOnce(Completion) -> Result<R>,
    ) -> Result<R> {
        self.records.dequeue(|completed| {
            let request = completed.result?;
            let result = match request.status_for(&self.renderer, &self.active) {
                Status::Complete(Ok(())) => {
                    self.scope
                        .with_renderer(&self.renderer, &self.active, |_| Ok(()))
                }
                Status::Complete(Err(error)) => Err(error),
                Status::Lost => Err(EIO),
                Status::Pending => return Err(EIO),
            };
            publish(Completion {
                use_id: completed.use_id,
                result,
                content: result.ok().and_then(|()| request.content_serial()),
                native: request.native_completion(),
            })
        })
    }

    /// Stop new demand and cancel accepted work. Busy means native retirement is pending;
    /// EIO means a lost claim remains quarantined. Neither result permits storage reuse.
    pub(crate) fn try_close(&mut self) -> Result {
        self.closing = true;
        self.changed.notify_all();
        self.records
            .for_each_pending(|pending| pending.request.cancel());
        self.advance();
        if self.lost {
            Err(EIO)
        } else if self.records.has_pending() {
            Err(EBUSY)
        } else {
            Ok(())
        }
    }
}

impl Drop for Queue {
    fn drop(&mut self) {
        let _ = self.try_close();
    }
}
