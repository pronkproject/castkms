// SPDX-License-Identifier: GPL-2.0-only

//! One bounded source stream tied to the endpoint's immutable accepted offer.

use super::{Endpoint, State};
use crate::renderer::{
    job::Completion,
    offer::Control,
    render_job::{RenderJob, Rendered},
};
use core::mem::MaybeUninit;
use kernel::{
    dma_fence::Fence,
    prelude::*,
    sync::{aref::ARef, Arc, UniqueArc},
};

enum Slot {
    Ready,
    Publishing { id: u64, image: u64 },
    Releasing { id: u64, image: u64 },
    Claimed {
        id: u64,
        image: u64,
        serial: u64,
        job: RenderJob,
        completed: UniqueArc<MaybeUninit<Rendered>>,
    },
}

pub(super) struct Stream {
    next_id: u64,
    last_serial: Option<u64>,
    last_released: Option<u64>,
    slot: Slot,
}

impl Stream {
    pub(super) fn new() -> Self {
        Self { next_id: 1, last_serial: None, last_released: None, slot: Slot::Ready }
    }

    pub(super) fn references(&self, image: u64) -> bool {
        match self.slot {
            Slot::Ready => false,
            Slot::Publishing { image: current, .. }
            | Slot::Releasing { image: current, .. }
            | Slot::Claimed { image: current, .. } => current == image,
        }
    }
}

impl Endpoint {
    /// Reserve independent private storage, then claim only the selected live offer's scene.
    /// All fallible encoding and descriptor preparation must finish before Pending::publish.
    pub(crate) fn begin_source(&self, image_id: u64) -> Result<Pending<'_>> {
        let mut state = self.state.lock();
        let State::Ready { offer, pool, source } = &mut *state else {
            return Err(if matches!(&*state, State::Closed) { EKEYREVOKED } else { ENODATA });
        };
        if !matches!(source.slot, Slot::Ready) {
            return Err(EBUSY);
        }
        let control = offer.control();
        control.with_current(|current| current.changed_content(source.last_serial).map(|_| ()))?;
        let image = pool.image(image_id)?;
        let id = source.next_id;
        source.next_id = id.checked_add(1).ok_or(EOVERFLOW)?;
        let retired = pool.withdraw(image_id)?;
        let previous = source.last_serial;
        source.slot = Slot::Publishing { id, image: image_id };
        drop(state);
        let mut pending = Pending {
            endpoint: self,
            control,
            id,
            image: image_id,
            serial: 0,
            job: None,
            completed: None,
        };
        drop(retired);
        pending.completed = Some(UniqueArc::new_uninit(GFP_KERNEL)?);
        pending.job = Some(pending.control.claim(image_id, previous, image.prepare(id)?)?);
        let job = pending.job.as_mut().ok_or(EIO)?;
        pending.serial = job.source().scene().content_serial().ok_or(ENODATA)?.get();
        job.observe_producers(&self.access.device().changed)?;
        Ok(pending)
    }

    /// Report source and private-write completion even after issuer authority is revoked.
    /// Closing the endpoint ends this reporting channel; it does not invent native completion.
    pub(crate) fn release_source(&self, id: u64, completion: Completion) -> Result {
        let (job, completed, image, serial) = {
            let mut state = self.state.lock();
            let State::Ready { source, .. } = &mut *state else {
                return Err(if matches!(&*state, State::Closed) { EKEYREVOKED } else { ENODATA });
            };
            if source.last_released == Some(id) {
                return Ok(());
            }
            match core::mem::replace(&mut source.slot, Slot::Ready) {
                Slot::Claimed { id: current, image, serial, job, completed } if current == id => {
                    source.last_released = Some(id);
                    source.slot = Slot::Releasing { id, image };
                    (job, completed, image, serial)
                }
                other => {
                    source.slot = other;
                    return Err(ENOENT);
                }
            }
        };
        let rendered = job.release(completion).map(|rendered| Arc::from(completed.write(rendered)));
        let mut state = self.state.lock();
        let State::Ready { source, pool, .. } = &mut *state else {
            drop(state);
            drop(rendered);
            self.access.device().changed.notify_all();
            return Ok(());
        };
        if !matches!(source.slot, Slot::Releasing { id: current, .. } if current == id) {
            drop(state);
            drop(rendered);
            return Err(ECANCELED);
        }
        let result = match rendered.as_ref() {
            Some(rendered) => {
                // Keep an outside-lock owner even if publication rejects this record.
                let result = pool.publish(image, rendered.clone());
                if result.is_ok() {
                    source.last_serial = Some(serial);
                }
                result
            }
            None => Ok(None),
        };
        source.slot = Slot::Ready;
        drop(state);
        drop(rendered);
        drop(result?);
        self.access.device().changed.notify_all();
        Ok(())
    }

    /// Borrow completed content with live offer admission; a recipient needs its own grant.
    pub(crate) fn completed_image(&self, id: u64) -> Result<Arc<Rendered>> {
        let state = self.state.lock();
        let State::Ready { offer, pool, .. } = &*state else {
            return Err(if matches!(&*state, State::Closed) { EKEYREVOKED } else { ENODATA });
        };
        let rendered = pool.completed(id)?;
        offer.control().check_completed(&rendered)?;
        Ok(rendered)
    }
}

/// An unpublished claim. Failure returns its slot and releases without renderer access.
#[must_use = "unpublished source claims must be published or dropped"]
pub(crate) struct Pending<'a> {
    endpoint: &'a Endpoint,
    control: Control,
    id: u64,
    image: u64,
    serial: u64,
    job: Option<RenderJob>,
    completed: Option<UniqueArc<MaybeUninit<Rendered>>>,
}

impl Pending<'_> {
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    pub(crate) fn scene_description(
        &self,
    ) -> Result<crate::renderer::description::Description<'_>> {
        crate::renderer::description::Description::new(
            self.job.as_ref().ok_or(EINVAL)?.source().scene(),
        )
    }

    pub(crate) fn producer_completion(&self) -> Result<Option<ARef<Fence>>> {
        self.job.as_ref().ok_or(EINVAL)?.source().producer_completion()
    }

    /// Install prepared files only while the exact entry and worker remain admitted.
    /// The transport finishes all fallible copyout first and supplies an infallible installer.
    pub(crate) fn publish(mut self, publish: impl FnOnce()) -> Result {
        let completed = self.completed.take().ok_or(EIO)?;
        let job = self.job.take().ok_or(EINVAL)?;
        let mut state = self.endpoint.state.lock();
        let State::Ready { source, .. } = &mut *state else {
            let error = if matches!(&*state, State::Closed) { EKEYREVOKED } else { ECANCELED };
            drop(state);
            job.release(Completion::WithoutAccess);
            return Err(error);
        };
        if !matches!(source.slot, Slot::Publishing { id, image }
            if id == self.id && image == self.image)
        {
            drop(state);
            job.release(Completion::WithoutAccess);
            return Err(ECANCELED);
        }
        if let Err(error) = self.control.publish_source(&job, publish) {
            drop(state);
            job.release(Completion::WithoutAccess);
            return Err(error);
        }
        source.slot = Slot::Claimed {
            id: self.id,
            image: self.image,
            serial: self.serial,
            job,
            completed,
        };
        Ok(())
    }
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        let job = self.job.take();
        let mut state = self.endpoint.state.lock();
        if let State::Ready { source, .. } = &mut *state {
            if matches!(source.slot, Slot::Publishing { id, .. } if id == self.id) {
                source.slot = Slot::Ready;
            }
        }
        drop(state);
        if let Some(job) = job {
            job.release(Completion::WithoutAccess);
        }
    }
}
