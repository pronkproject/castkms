// SPDX-License-Identifier: GPL-2.0-only

//! One renderer endpoint's candidate and active ownership, independent of file transport.

mod output;
use output::Slot as OutputSlot;

use super::{
    candidate::Candidate,
    job::Completion,
    permission::Access,
    private_pool::Pool,
    probe::Source as ProbeSource, //
    render_job::{RenderJob, Rendered},
};
use crate::{
    execution::Description,
    renderer_startup,
    scene::Configuration,
    Driver, //
};
use core::mem::MaybeUninit;
use kernel::{
    dma_buf::DmaBuf,
    dma_fence::Fence,
    drm::device::RegisteredDeviceRef,
    prelude::*,
    sync::{aref::ARef, Arc, Mutex, UniqueArc}, //
};

enum Slot {
    Idle,
    Host {
        id: u64,
        description: Description,
    },
    Publishing,
    Active {
        id: u64,
        candidate: Arc<Candidate>,
    },
    Activating {
        id: u64,
        candidate: Arc<Candidate>,
    },
    Renderer {
        id: u64,
        candidate: Arc<Candidate>,
        active: renderer_startup::Active,
        route: super::routing::Owner,
        _source: ProbeSource,
        description: Description,
        next_source_id: u64,
        last_content_serial: Option<u64>,
        last_released_source: Option<u64>,
        source: SourceSlot,
        output: OutputSlot,
        next_output_id: u64,
        last_released_output: Option<u64>,
    },
}

enum SourceSlot {
    Ready,
    Publishing {
        id: u64,
        image: u64,
    },
    Releasing {
        id: u64,
        image: u64,
    },
    Claimed {
        id: u64,
        image: u64,
        job: RenderJob,
        completed: UniqueArc<MaybeUninit<Rendered>>,
    },
}

struct State {
    closed: bool,
    next_id: u64,
    slot: Slot,
    proposal: Option<Arc<super::proposal::Proposal>>,
    images: Option<Pool>,
}

#[pin_data]
pub(crate) struct Session {
    access: Access,
    device: RegisteredDeviceRef<Driver>,
    #[pin]
    state: Mutex<State>,
}

impl Session {
    pub(crate) fn new(access: Access, device: RegisteredDeviceRef<Driver>) -> Result<Arc<Self>> {
        Arc::pin_init(
            try_pin_init!(Self {
                access,
                device,
                state <- kernel::new_mutex!(State {
                    closed: false,
                    next_id: 1,
                    slot: Slot::Idle,
                    proposal: None,
                    images: Some(Pool::new()?),
                }),
            }),
            GFP_KERNEL,
        )
    }

    pub(crate) fn description(&self) -> Result<Description> {
        self.access
            .with_output(|| Ok(self.access.display().execution.describe()))
    }

    pub(crate) fn capabilities(&self) -> Result<crate::execution::publication::CapabilitySnapshot> {
        self.access.with_output(|| {
            let device = self.access.device();
            let output = device
                .displays
                .iter()
                .position(|display| core::ptr::eq(&**display, self.access.display()))
                .ok_or(EINVAL)?;
            self.access
                .display()
                .execution
                .capabilities(&device.validation, output)
        })
    }

    /// Retain one proposed profile independently of the file transport's reply.
    pub(crate) fn propose_profile(
        &self,
        id: u64,
        profile: crate::execution::capabilities::Profile,
    ) -> Result<crate::execution::proposal::DescriptionSnapshot> {
        self.propose(id, Some(profile))
    }

    pub(crate) fn propose_host(
        &self,
        id: u64,
    ) -> Result<crate::execution::proposal::DescriptionSnapshot> {
        self.propose(id, None)
    }

    fn propose(
        &self,
        id: u64,
        profile: Option<crate::execution::capabilities::Profile>,
    ) -> Result<crate::execution::proposal::DescriptionSnapshot> {
        let candidate = self.candidate(id)?;
        let proposal = Arc::new(
            match profile {
                Some(profile) => candidate.propose_profile(profile)?,
                None => candidate.propose_host()?,
            },
            GFP_KERNEL,
        )?;
        let description = proposal.describe().clone();
        let mut state = self.state.lock();
        if state.closed {
            return Err(EKEYREVOKED);
        }
        match &state.slot {
            Slot::Active {
                id: current,
                candidate: current_candidate,
            } if *current == id && Arc::ptr_eq(current_candidate, &candidate) => (),
            _ => return Err(ESTALE),
        }
        if state.proposal.is_some() {
            return Err(EBUSY);
        }
        state.proposal = Some(proposal);
        drop(state);
        self.notify_capabilities();
        Ok(description)
    }

    fn notify_capabilities(&self) {
        if let Some(registered) = self.device.registration_guard() {
            registered.hotplug_event();
        }
    }

    /// A current pending observation for reconciliation, not permission to activate it.
    #[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
    pub(crate) fn pending_profile(
        &self,
    ) -> Result<Option<crate::execution::proposal::DescriptionSnapshot>> {
        self.access
            .with_output(|| Ok(self.access.display().execution.pending_profile()))
    }

    pub(crate) fn begin(&self, expected_generation: u64) -> Result<Pending<'_>> {
        {
            let state = self.state.lock();
            if state.closed {
                return Err(EKEYREVOKED);
            }
            if !matches!(state.slot, Slot::Idle | Slot::Host { .. }) {
                return Err(EBUSY);
            }
        }
        let candidate = Arc::new(Candidate::begin(self.access.clone())?, GFP_KERNEL)?;
        let id = {
            let mut state = self.state.lock();
            if state.closed {
                return Err(EKEYREVOKED);
            }
            if !matches!(state.slot, Slot::Idle | Slot::Host { .. }) {
                return Err(EBUSY);
            }
            let id = state.next_id;
            state.next_id = id.checked_add(1).ok_or(EOVERFLOW)?;
            state.slot = Slot::Publishing;
            id
        };
        let pending = Pending {
            session: self,
            id,
            candidate: Some(candidate),
        };
        if pending.execution()?.generation != expected_generation {
            return Err(ESTALE);
        }
        pending.candidate()?.validate()?;
        Ok(pending)
    }

    pub(crate) fn abort(&self, id: u64) -> Result {
        let (candidate, proposal) = {
            let mut state = self.state.lock();
            let candidate = match core::mem::replace(&mut state.slot, Slot::Idle) {
                Slot::Active {
                    id: current,
                    candidate,
                } if current == id => candidate,
                other @ Slot::Active { .. } => {
                    state.slot = other;
                    return Err(ENOENT);
                }
                Slot::Publishing => {
                    state.slot = Slot::Publishing;
                    return Err(EBUSY);
                }
                other @ Slot::Activating { .. } => {
                    state.slot = other;
                    return Err(EBUSY);
                }
                other @ Slot::Renderer { id: current, .. } => {
                    state.slot = other;
                    return if current == id {
                        Err(EALREADY)
                    } else {
                        Err(ENOENT)
                    };
                }
                other @ Slot::Host { id: current, .. } => {
                    state.slot = other;
                    return if current == id {
                        Err(EALREADY)
                    } else {
                        Err(ENOENT)
                    };
                }
                Slot::Idle => return Err(ENOENT),
            };
            (candidate, state.proposal.take())
        };
        candidate.cancel();
        drop(candidate);
        let changed = proposal.is_some();
        drop(proposal);
        if changed {
            self.notify_capabilities();
        }
        Ok(())
    }

    /// Retain the named candidate for a pixel operation without holding the session lock.
    pub(crate) fn candidate(&self, id: u64) -> Result<Arc<Candidate>> {
        let state = self.state.lock();
        if state.closed {
            return Err(EKEYREVOKED);
        }
        match &state.slot {
            Slot::Active {
                id: current,
                candidate,
            } if *current == id => Ok(candidate.clone()),
            Slot::Publishing | Slot::Activating { .. } => Err(EBUSY),
            Slot::Renderer { id: current, .. } if *current == id => Err(EALREADY),
            Slot::Host { id: current, .. } if *current == id => Err(EALREADY),
            _ => Err(ENOENT),
        }
    }

    /// Activate one completed candidate or reconcile an already published result.
    pub(crate) fn activate(&self, id: u64) -> Result<Description> {
        let registered = self.device.registration_guard().ok_or(ENODEV)?;
        let (candidate, proposal) = {
            let mut state = self.state.lock();
            if state.closed {
                return Err(EKEYREVOKED);
            }
            if let Slot::Host {
                id: current,
                description,
            } = &state.slot
            {
                return if *current == id {
                    self.access.with_output(|| {
                        if self.access.display().execution.describe() == *description {
                            Ok(*description)
                        } else {
                            Err(ESTALE)
                        }
                    })
                } else {
                    Err(ENOENT)
                };
            }
            if let Slot::Renderer {
                id: current,
                active,
                description,
                ..
            } = &state.slot
            {
                return if *current == id {
                    active.check()?;
                    if self.access.display().execution.describe() == *description {
                        Ok(*description)
                    } else {
                        Err(EIO)
                    }
                } else {
                    Err(ENOENT)
                };
            }
            match core::mem::replace(&mut state.slot, Slot::Idle) {
                Slot::Active {
                    id: current,
                    candidate,
                } if current == id => {
                    let Some(proposal) = state.proposal.clone() else {
                        state.slot = Slot::Active { id, candidate };
                        return Err(EINVAL);
                    };
                    state.slot = Slot::Activating {
                        id,
                        candidate: candidate.clone(),
                    };
                    (candidate, proposal)
                }
                other @ Slot::Publishing | other @ Slot::Activating { .. } => {
                    state.slot = other;
                    return Err(EBUSY);
                }
                other => {
                    state.slot = other;
                    return Err(ENOENT);
                }
            }
        };

        let activated = if matches!(
            proposal.describe().profile,
            crate::execution::validation::Contract::Host
        ) {
            proposal
                .handback(&registered)
                .map(|description| (None, description))
        } else {
            super::routing::Prepared::new().and_then(|prepared| {
                proposal.activate(&registered).map(|(active, source, description)| {
                    (Some((active, source, prepared)), description)
                })
            })
        };
        let mut state = self.state.lock();
        if state.closed {
            state.slot = Slot::Idle;
            drop(state);
            drop(activated);
            return Err(EKEYREVOKED);
        }
        if !matches!(state.slot, Slot::Activating { id: current, .. } if current == id) {
            drop(state);
            drop(activated);
            return Err(ECANCELED);
        }
        match activated {
            Ok((None, description)) => {
                state.slot = Slot::Host { id, description };
                let retired = state.proposal.take();
                drop(state);
                drop(retired);
                registered.hotplug_event();
                Ok(description)
            }
            Ok((Some((active, source, prepared)), description)) => {
                let route = match self.access.display().renderer_routes.publish(
                    prepared,
                    &candidate,
                    &active,
                ) {
                    Ok(route) => route,
                    Err(error) => {
                        // Execution was published, but shutdown or replacement won discovery.
                        // The candidate cannot be restored as an unactivated reservation.
                        state.slot = Slot::Idle;
                        let retired = state.proposal.take();
                        drop(state);
                        drop((active, source, candidate, retired));
                        return Err(error);
                    }
                };
                state.slot = Slot::Renderer {
                    id,
                    candidate,
                    active,
                    route,
                    _source: source,
                    description,
                    next_source_id: 1,
                    last_content_serial: None,
                    last_released_source: None,
                    source: SourceSlot::Ready,
                    output: OutputSlot::Ready,
                    next_output_id: 1,
                    last_released_output: None,
                };
                drop(state);
                registered.hotplug_event();
                Ok(description)
            }
            Err(error) => {
                state.slot = Slot::Active { id, candidate };
                Err(error)
            }
        }
    }

    /// Register private backing under an increasing endpoint-local name.
    pub(crate) fn register_image(
        &self,
        id: u64,
        dimensions: [u32; 2],
        buffers: &[ARef<DmaBuf>],
    ) -> Result {
        let mut state = self.state.lock();
        if state.closed {
            return Err(EKEYREVOKED);
        }
        let State { slot, images, .. } = &mut *state;
        let Slot::Renderer {
            candidate, active, ..
        } = slot
        else {
            return Err(EOPNOTSUPP);
        };
        images.as_mut().ok_or(ESHUTDOWN)?.insert(id, || {
            candidate.register_private_image(active, dimensions, buffers)
        })
    }

    /// Forget a name without completing native work. Claimed or publishing source jobs
    /// must be released first; submitted native accesses retain their own registration.
    pub(crate) fn unregister_image(&self, id: u64) -> Result {
        let retired = {
            let mut state = self.state.lock();
            if let Slot::Renderer { source, .. } = &state.slot {
                match source {
                    SourceSlot::Publishing { image, .. }
                    | SourceSlot::Releasing { image, .. }
                    | SourceSlot::Claimed { image, .. }
                        if *image == id =>
                    {
                        return Err(EBUSY)
                    }
                    _ => (),
                }
            }
            state.images.as_mut().ok_or(ESHUTDOWN)?.remove(id)?
        };
        drop(retired);
        self.access.device().changed.notify_all();
        Ok(())
    }

    /// Borrow retained private storage, not an output claim or proof of pixel validity.
    /// Output admission must intersect the image evidence with live recipient authority.
    #[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
    pub(crate) fn completed_image(&self, id: u64) -> Result<Arc<Rendered>> {
        let state = self.state.lock();
        if state.closed {
            return Err(EKEYREVOKED);
        }
        let Slot::Renderer {
            candidate, active, ..
        } = &state.slot
        else {
            return Err(EOPNOTSUPP);
        };
        candidate.with_observed_control(&active.observation(), |_| {
            state.images.as_ref().ok_or(ESHUTDOWN)?.completed(id)
        })
    }

    /// Reserve one changed scene and its private destination before descriptor publication.
    pub(crate) fn begin_source(&self, image_id: u64) -> Result<PendingSource<'_>> {
        let mut state = self.state.lock();
        if state.closed {
            return Err(EKEYREVOKED);
        }
        let State { slot, images, .. } = &mut *state;
        let Slot::Renderer {
            candidate,
            active,
            description,
            next_source_id,
            last_content_serial,
            source,
            ..
        } = slot
        else {
            return if matches!(slot, Slot::Publishing | Slot::Activating { .. }) {
                Err(EBUSY)
            } else {
                Err(EOPNOTSUPP)
            };
        };
        if !matches!(source, SourceSlot::Ready) {
            return Err(EBUSY);
        }
        candidate.with_observed_control(&active.observation(), |current| {
            current.changed_content(*last_content_serial).map(|_| ())
        })?;
        let images = images.as_mut().ok_or(ESHUTDOWN)?;
        let image = images.image(image_id)?;
        let id = *next_source_id;
        *next_source_id = id.checked_add(1).ok_or(EOVERFLOW)?;
        let retired = images.withdraw(image_id)?;
        let candidate = candidate.clone();
        let active = active.observation();
        let description = *description;
        let previous = *last_content_serial;
        *source = SourceSlot::Publishing {
            id,
            image: image_id,
        };
        drop(state);
        let mut pending = PendingSource {
            session: self,
            id,
            image: image_id,
            content_serial: 0,
            job: None,
            completed: None,
        };
        drop(retired);
        pending.completed = Some(UniqueArc::new_uninit(GFP_KERNEL)?);
        pending.job = Some(candidate.claim_render_observed(
            &active,
            description,
            previous,
            image.prepare(id)?,
        )?);
        let job = pending.job.as_mut().ok_or(EIO)?;
        pending.content_serial = job.source().scene().content_serial().ok_or(ENODATA)?.get();
        job.observe_producers(&self.access.device().changed)?;
        Ok(pending)
    }

    /// Resolve one published source job, accepting a repeated terminal record.
    pub(crate) fn release_source(&self, id: u64, completion: Completion) -> Result {
        let (job, completed, image) = {
            let mut state = self.state.lock();
            if state.closed {
                return Err(EKEYREVOKED);
            }
            let Slot::Renderer {
                source,
                last_released_source,
                ..
            } = &mut state.slot
            else {
                return Err(EOPNOTSUPP);
            };
            if *last_released_source == Some(id) {
                return Ok(());
            }
            match core::mem::replace(source, SourceSlot::Ready) {
                SourceSlot::Claimed {
                    id: current,
                    image,
                    job,
                    completed,
                } if current == id => {
                    *last_released_source = Some(id);
                    *source = SourceSlot::Releasing { id, image };
                    (job, completed, image)
                }
                other => {
                    *source = other;
                    return Err(ENOENT);
                }
            }
        };
        let rendered = job
            .release(completion)
            .map(|rendered| Arc::from(completed.write(rendered)));
        let mut state = self.state.lock();
        let State { slot, images, .. } = &mut *state;
        let publication = match slot {
            Slot::Renderer { source, .. } if matches!(&*source, SourceSlot::Releasing { id: current, .. } if *current == id) =>
            {
                let result = match rendered {
                    Some(rendered) => match images.as_mut() {
                        Some(images) => images.publish(image, rendered),
                        None => Err(ESHUTDOWN),
                    },
                    None => Ok(None),
                };
                *source = SourceSlot::Ready;
                result
            }
            _ => {
                drop(state);
                drop(rendered);
                self.access.device().changed.notify_all();
                return Ok(());
            }
        };
        drop(state);
        drop(publication?);
        self.access.device().changed.notify_all();
        Ok(())
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(crate) fn close_for_test(&self) {
        self.close();
    }

    pub(super) fn close(&self) {
        let (slot, proposal, images) = {
            let mut state = self.state.lock();
            state.closed = true;
            (
                core::mem::replace(&mut state.slot, Slot::Idle),
                state.proposal.take(),
                state.images.take(),
            )
        };
        match &slot {
            Slot::Active { candidate, .. }
            | Slot::Activating { candidate, .. }
            | Slot::Renderer { candidate, .. } => candidate.cancel(),
            _ => (),
        }
        // Both job families and routing release outside endpoint exclusion. Unreported
        // published access remains quarantined; pending publishers retain their own claims.
        drop(slot);
        drop(images);
        let changed = proposal.is_some();
        drop(proposal);
        if changed {
            self.notify_capabilities();
        }
    }
}

/// One claimed job not yet visible through its transport.
#[must_use = "dropping an unpublished source job returns its queue slot"]
pub(crate) struct PendingSource<'a> {
    session: &'a Session,
    id: u64,
    image: u64,
    content_serial: u64,
    job: Option<RenderJob>,
    completed: Option<UniqueArc<MaybeUninit<Rendered>>>,
}

impl PendingSource<'_> {
    pub(crate) fn scene_description(&self) -> Result<super::description::Description<'_>> {
        super::description::Description::new(self.job.as_ref().ok_or(EINVAL)?.source().scene())
    }

    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    pub(crate) fn producer_completion(&self) -> Result<Option<kernel::sync::aref::ARef<Fence>>> {
        self.job
            .as_ref()
            .ok_or(EINVAL)?
            .source()
            .producer_completion()
    }

    /// Publish the job after an adapter has completed every fallible operation.
    pub(crate) fn publish(mut self, publish: impl FnOnce()) -> Result {
        let completed = self.completed.take().ok_or(EIO)?;
        let job = self.job.take().ok_or(EINVAL)?;
        let mut state = self.session.state.lock();
        if state.closed {
            drop(state);
            job.release(Completion::WithoutAccess);
            return Err(EKEYREVOKED);
        }
        let Slot::Renderer {
            source,
            last_content_serial,
            ..
        } = &mut state.slot
        else {
            drop(state);
            job.release(Completion::WithoutAccess);
            return Err(ECANCELED);
        };
        if !matches!(source, SourceSlot::Publishing { id, image } if *id == self.id && *image == self.image)
        {
            drop(state);
            job.release(Completion::WithoutAccess);
            return Err(ECANCELED);
        }
        publish();
        *last_content_serial = Some(self.content_serial);
        *source = SourceSlot::Claimed {
            id: self.id,
            image: self.image,
            job,
            completed,
        };
        Ok(())
    }
}

impl Drop for PendingSource<'_> {
    fn drop(&mut self) {
        let job = self.job.take();
        let mut state = self.session.state.lock();
        if let Slot::Renderer { source, .. } = &mut state.slot {
            if matches!(source, SourceSlot::Publishing { id, .. } if *id == self.id) {
                *source = SourceSlot::Ready;
            }
        }
        drop(state);
        if let Some(job) = job {
            job.release(Completion::WithoutAccess);
        }
    }
}

#[must_use = "dropping an unpublished candidate cancels its reservation"]
pub(crate) struct Pending<'a> {
    session: &'a Session,
    id: u64,
    candidate: Option<Arc<Candidate>>,
}

impl Pending<'_> {
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    pub(crate) fn configuration(&self) -> Result<&Configuration> {
        Ok(self.candidate()?.configuration())
    }

    pub(crate) fn execution(&self) -> Result<Description> {
        Ok(self.candidate()?.execution())
    }

    fn candidate(&self) -> Result<&Arc<Candidate>> {
        self.candidate.as_ref().ok_or(EINVAL)
    }

    pub(crate) fn publish(mut self) -> Result {
        self.candidate()?.validate()?;
        let candidate = self.candidate.take().ok_or(EINVAL)?;
        let mut state = self.session.state.lock();
        if state.closed {
            drop(state);
            candidate.cancel();
            drop(candidate);
            return Err(EKEYREVOKED);
        }
        if !matches!(state.slot, Slot::Publishing) {
            drop(state);
            candidate.cancel();
            drop(candidate);
            return Err(ECANCELED);
        }
        state.slot = Slot::Active {
            id: self.id,
            candidate,
        };
        Ok(())
    }
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        if let Some(candidate) = self.candidate.take() {
            let mut state = self.session.state.lock();
            if matches!(state.slot, Slot::Publishing) {
                state.slot = Slot::Idle;
            }
            drop(state);
            candidate.cancel();
            drop(candidate);
        }
    }
}
