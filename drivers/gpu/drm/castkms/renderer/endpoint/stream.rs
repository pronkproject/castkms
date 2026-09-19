// SPDX-License-Identifier: GPL-2.0-only

//! One bounded source stream tied to the endpoint's immutable accepted publication.

use super::{Endpoint, Ordering, State};
use crate::renderer::{
    job::Completion,
    publication::Control,
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
    pub(super) fn new(next_id: u64) -> Self {
        Self { next_id, last_serial: None, last_released: None, slot: Slot::Ready }
    }

    pub(super) fn references(&self, image: u64) -> bool {
        match self.slot {
            Slot::Ready => false,
            Slot::Publishing { image: current, .. }
            | Slot::Releasing { image: current, .. }
            | Slot::Claimed { image: current, .. } => current == image,
        }
    }

    pub(super) fn idle(&self) -> bool {
        matches!(self.slot, Slot::Ready)
    }
}

impl Endpoint {
    /// Advisory source availability. An unselected publication is idle, not revoked.
    pub(crate) fn source_readable(&self) -> Result<bool> {
        if self.access.is_revoked() {
            return Err(EKEYREVOKED);
        }
        let interval = self.refresh_generation()?;
        let state = self.state.lock();
        match &*state {
            State::Closed => Err(EKEYREVOKED),
            State::Ready { publication, source, .. } => {
                // A claim from an earlier master interval can keep the retired
                // publication attached until its release. The endpoint can be
                // configured again afterward, so the drain is idle, not terminal.
                if publication.interval() != interval {
                    return Ok(false);
                }
                if !publication.is_live() {
                    return Err(EKEYREVOKED);
                }
                if !matches!(source.slot, Slot::Ready) {
                    return Ok(false);
                }
                match publication.control().with_current(|current| {
                    current.changed_content(source.last_serial).map(|_| true)
                }) {
                    Err(ESTALE | EAGAIN | ENODATA | ENODEV) => Ok(false),
                    result => result,
                }
            }
            _ => Ok(false),
        }
    }

    /// Reserve independent private storage, then claim only the selected live publication's scene.
    /// All fallible encoding and descriptor preparation must finish before Pending::publish.
    pub(crate) fn begin_source(&self, image_id: u64) -> Result<Pending<'_>> {
        self.refresh_generation()?;
        let mut state = self.state.lock();
        let State::Ready { publication, pool, source, .. } = &mut *state else {
            return Err(if matches!(&*state, State::Closed) { EKEYREVOKED } else { ENODATA });
        };
        if !matches!(source.slot, Slot::Ready) {
            return Err(EBUSY);
        }
        let control = publication.control();
        control.with_current(|current| current.changed_content(source.last_serial).map(|_| ()))?;
        let image = pool.image(image_id)?;
        let id = source.next_id;
        source.next_id = id.checked_add(1).ok_or(EOVERFLOW)?;
        self.next_source_id.store(source.next_id, Ordering::Relaxed);
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
            failed_producer: false,
            job: None,
            completed: None,
        };
        drop(retired);
        pending.completed = Some(UniqueArc::new_uninit(GFP_KERNEL)?);
        pending.job = Some(pending.control.claim(image_id, previous, image.prepare(id)?)?);
        let job = pending.job.as_mut().ok_or(EIO)?;
        pending.serial = job.source().scene().render_content().ok_or(ENODATA)?.get();
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

    /// Borrow completed content with live publication admission; a recipient needs its own grant.
    pub(crate) fn completed_image(&self, id: u64) -> Result<Arc<Rendered>> {
        self.refresh_generation()?;
        let state = self.state.lock();
        let State::Ready { publication, pool, .. } = &*state else {
            return Err(if matches!(&*state, State::Closed) { EKEYREVOKED } else { ENODATA });
        };
        let rendered = pool.completed(id)?;
        publication.control().check_completed(&rendered)?;
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
    failed_producer: bool,
    job: Option<RenderJob>,
    completed: Option<UniqueArc<MaybeUninit<Rendered>>>,
}

impl Pending<'_> {
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    pub(crate) fn constraints_id(&self) -> u64 {
        self.control.entry().id()
    }

    pub(crate) fn scene_description(
        &self,
    ) -> Result<crate::renderer::description::Description<'_>> {
        crate::renderer::description::Description::new(
            self.job.as_ref().ok_or(EINVAL)?.source().scene(),
        )
    }

    /// A terminal producer error discards this serial when the unpublished claim
    /// is dropped, so polling cannot repeatedly report the same unusable scene.
    pub(crate) fn producer_completion(&mut self) -> Result<Option<ARef<Fence>>> {
        let result = self.job.as_ref().ok_or(EINVAL)?.source().producer_completion();
        self.failed_producer |= result.as_ref().err() == Some(&EREMOTEIO);
        result
    }

    /// Install prepared files only while the exact entry and worker remain admitted.
    /// The transport finishes all fallible copyout first and supplies an infallible installer.
    pub(crate) fn publish(mut self, publish: impl FnOnce()) -> Result {
        if self.failed_producer {
            return Err(EREMOTEIO);
        }
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
                if self.failed_producer {
                    source.last_serial = Some(self.serial);
                }
                source.slot = Slot::Ready;
            }
        }
        drop(state);
        if let Some(job) = job {
            job.release(Completion::WithoutAccess);
        }
        self.endpoint.changed().notify_all();
    }
}
