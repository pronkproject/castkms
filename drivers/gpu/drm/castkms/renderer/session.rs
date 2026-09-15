// SPDX-License-Identifier: GPL-2.0-only

//! One renderer endpoint's candidate and active ownership, independent of file transport.

use super::{
    candidate::Candidate,
    job::{Completion, Description as SourceDescription, Plane, SourceJob},
    permission::Access,
    probe::Source as ProbeSource, //
};
use crate::{
    execution::Description,
    renderer_startup,
    scene::Configuration,
    Driver, //
};
use kernel::{
    dma_fence::Fence,
    drm::device::RegisteredDeviceRef,
    prelude::*,
    sync::{Arc, Mutex}, //
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
        _source: ProbeSource,
        description: Description,
        next_source_id: u64,
        last_content_serial: Option<u64>,
        last_released_source: Option<u64>,
        source: SourceSlot,
    },
}

enum SourceSlot {
    Ready,
    Publishing(u64),
    Claimed { id: u64, job: SourceJob },
}

struct State {
    closed: bool,
    next_id: u64,
    slot: Slot,
    proposal: Option<Arc<super::proposal::Proposal>>,
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
            pin_init!(Self {
                access,
                device,
                state <- kernel::new_mutex!(State {
                    closed: false,
                    next_id: 1,
                    slot: Slot::Idle,
                    proposal: None,
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
        Ok(description)
    }

    /// A current pending observation for reconciliation, not permission to activate it.
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
        drop(proposal);
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
                    state.slot = Slot::Activating {
                        id,
                        candidate: candidate.clone(),
                    };
                    (candidate, state.proposal.clone())
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

        let activated = match proposal {
            Some(proposal)
                if matches!(
                    proposal.describe().profile,
                    crate::execution::validation::Contract::Host
                ) =>
            {
                proposal
                    .handback(&registered)
                    .map(|description| (None, description))
            }
            Some(proposal) => proposal
                .activate(&registered)
                .map(|(active, source, description)| (Some((active, source)), description)),
            None => candidate
                .activate(&registered)
                .map(|(active, source, description)| (Some((active, source)), description)),
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
            Ok((Some((active, source)), description)) => {
                state.slot = Slot::Renderer {
                    id,
                    candidate,
                    active,
                    _source: source,
                    description,
                    next_source_id: 1,
                    last_content_serial: None,
                    last_released_source: None,
                    source: SourceSlot::Ready,
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

    /// Reserve one changed current scene for publication by a transport adapter.
    pub(crate) fn begin_source(&self) -> Result<PendingSource<'_>> {
        let mut state = self.state.lock();
        if state.closed {
            return Err(EKEYREVOKED);
        }
        let Slot::Renderer {
            candidate,
            active,
            description,
            next_source_id,
            last_content_serial,
            source,
            ..
        } = &mut state.slot
        else {
            return if matches!(state.slot, Slot::Publishing | Slot::Activating { .. }) {
                Err(EBUSY)
            } else {
                Err(EOPNOTSUPP)
            };
        };
        if !matches!(source, SourceSlot::Ready) {
            return Err(EBUSY);
        }
        let job = candidate.claim_source(active, *description, *last_content_serial)?;
        let content_serial = match job.scene().content_serial() {
            Some(content) => content.get(),
            None => {
                drop(state);
                job.release_without_access();
                return Err(ENODATA);
            }
        };
        if *last_content_serial == Some(content_serial) {
            drop(state);
            job.release_without_access();
            return Err(ENODATA);
        }
        let id = *next_source_id;
        *next_source_id = match id.checked_add(1) {
            Some(next) => next,
            None => {
                drop(state);
                job.release_without_access();
                return Err(EOVERFLOW);
            }
        };
        *source = SourceSlot::Publishing(id);
        Ok(PendingSource {
            session: self,
            id,
            content_serial,
            job: Some(job),
        })
    }

    /// Resolve one published source job, accepting a repeated terminal record.
    pub(crate) fn release_source(&self, id: u64, completion: Completion) -> Result {
        let job = {
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
                SourceSlot::Claimed { id: current, job } if current == id => {
                    *last_released_source = Some(id);
                    job
                }
                other => {
                    *source = other;
                    return Err(ENOENT);
                }
            }
        };
        job.release(completion);
        Ok(())
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(crate) fn close_for_test(&self) {
        self.close();
    }

    pub(super) fn close(&self) {
        let (candidate, active, source, proposal) = {
            let mut state = self.state.lock();
            state.closed = true;
            let (candidate, active, source) = match core::mem::replace(&mut state.slot, Slot::Idle)
            {
                Slot::Active { candidate, .. } | Slot::Activating { candidate, .. } => {
                    (Some(candidate), None, None)
                }
                Slot::Renderer {
                    candidate,
                    active,
                    source,
                    ..
                } => (
                    Some(candidate),
                    Some(active),
                    match source {
                        SourceSlot::Claimed { job, .. } => Some(job),
                        SourceSlot::Ready | SourceSlot::Publishing(_) => None,
                    },
                ),
                _ => (None, None, None),
            };
            (candidate, active, source, state.proposal.take())
        };
        if let Some(candidate) = candidate {
            candidate.cancel();
            drop(candidate);
        }
        drop(source);
        drop(active);
        drop(proposal);
    }
}

/// One claimed job not yet visible through its transport.
#[must_use = "dropping an unpublished source job returns its queue slot"]
pub(crate) struct PendingSource<'a> {
    session: &'a Session,
    id: u64,
    content_serial: u64,
    job: Option<SourceJob>,
}

impl PendingSource<'_> {
    pub(crate) fn scene_description(&self) -> Result<super::description::Description<'_>> {
        super::description::Description::new(self.job.as_ref().ok_or(EINVAL)?.scene())
    }

    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    pub(crate) fn description(&self) -> Result<SourceDescription> {
        self.job.as_ref().ok_or(EINVAL)?.description()
    }

    pub(crate) fn plane(&self, index: usize) -> Result<Plane<'_>> {
        self.job.as_ref().ok_or(EINVAL)?.plane(index)
    }

    pub(crate) fn producer_completion(&self) -> Result<Option<kernel::sync::aref::ARef<Fence>>> {
        self.job.as_ref().ok_or(EINVAL)?.producer_completion()
    }

    /// Publish the job after an adapter has completed every fallible operation.
    pub(crate) fn publish(mut self, publish: impl FnOnce()) -> Result {
        let job = self.job.take().ok_or(EINVAL)?;
        let mut state = self.session.state.lock();
        if state.closed {
            drop(state);
            job.release_without_access();
            return Err(EKEYREVOKED);
        }
        let Slot::Renderer {
            source,
            last_content_serial,
            ..
        } = &mut state.slot
        else {
            drop(state);
            job.release_without_access();
            return Err(ECANCELED);
        };
        if !matches!(source, SourceSlot::Publishing(id) if *id == self.id) {
            drop(state);
            job.release_without_access();
            return Err(ECANCELED);
        }
        publish();
        *last_content_serial = Some(self.content_serial);
        *source = SourceSlot::Claimed { id: self.id, job };
        Ok(())
    }
}

impl Drop for PendingSource<'_> {
    fn drop(&mut self) {
        let Some(job) = self.job.take() else {
            return;
        };
        let mut state = self.session.state.lock();
        if let Slot::Renderer { source, .. } = &mut state.slot {
            if matches!(source, SourceSlot::Publishing(id) if *id == self.id) {
                *source = SourceSlot::Ready;
            }
        }
        drop(state);
        job.release_without_access();
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
