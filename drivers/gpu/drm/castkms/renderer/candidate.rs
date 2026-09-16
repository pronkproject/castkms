// SPDX-License-Identifier: GPL-2.0-only

//! Authorized private startup for one accepted display configuration.

use super::{
    permission::Access,
    private_image::Image,
    probe::{Probe, Source as ProbeSource},
};
use crate::{
    display_control,
    execution::{Description, Profile},
    host_compositor::compose::Completed,
    host_snapshot::Snapshot,
    image_access,
    renderer::job::SourceJob,
    renderer_startup,
    scene::Configuration,
    Driver, //
};
use kernel::{
    dma_buf::DmaBuf,
    dma_fence::Fence,
    drm::{
        device::Registered,
        kms::LockedState,
        Device, //
    },
    prelude::*, //
    sync::{aref::ARef, Arc},
};

/// One renderer's private reservation, without an activation or live-source claim.
///
/// Configuration equality denotes the same mode and route interval, not equal dimensions.
/// Content-only updates do not invalidate startup. The access handle retains its issuer's
/// revocation state, not its issuer lifetime. Drop outside native DRM and admission locks.
#[must_use = "dropping the candidate releases its private startup reservation"]
pub(crate) struct Candidate {
    resources: renderer_startup::Candidate,
    access: Access,
    configuration: Configuration,
    execution: Description,
    probe: Arc<Probe>,
    proposal_owner: Arc<()>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Candidate {
    /// Reserve outside policy locks, checking the configuration on both sides.
    pub(crate) fn begin(access: Access) -> Result<Self> {
        Self::begin_then(access, || Ok(()))
    }

    fn begin_then(access: Access, after_reserve: impl FnOnce() -> Result) -> Result<Self> {
        let probe = Arc::pin_init(Probe::new(), GFP_KERNEL)?;
        let proposal_owner = Arc::new((), GFP_KERNEL)?;
        let configuration = access.with_current(|current| Ok(current.configuration().clone()))?;
        let execution = access.display().execution.describe();
        let resources = access.display().startup.begin()?;
        let candidate = Self {
            resources,
            access,
            configuration,
            execution,
            probe,
            proposal_owner,
        };
        after_reserve()?;
        candidate.with_current_control(|_| {
            candidate
                .access
                .display()
                .execution
                .retire_other_worker_proposal(&candidate.proposal_owner);
            Ok(())
        })?;
        Ok(candidate)
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(crate) fn begin_then_for_test(
        access: Access,
        after_reserve: impl FnOnce() -> Result,
    ) -> Result<Self> {
        Self::begin_then(access, after_reserve)
    }

    /// Historical configuration metadata, not permission to activate or read pixels.
    pub(crate) fn configuration(&self) -> &Configuration {
        &self.configuration
    }

    /// Observe the execution description captured with this startup reservation.
    pub(crate) fn execution(&self) -> Description {
        self.execution
    }

    /// Register immutable pending capabilities without changing KMS acceptance.
    /// The profile is allocated before entering authority and startup exclusion.
    pub(crate) fn propose_profile(
        self: &Arc<Self>,
        profile: crate::execution::capabilities::Profile,
    ) -> Result<super::proposal::Proposal> {
        let profile = Arc::new(profile, GFP_KERNEL)?;
        let target = crate::execution::validation::Contract::Renderer(profile.clone());
        self.propose_contract(target)
    }

    pub(crate) fn propose_host(self: &Arc<Self>) -> Result<super::proposal::Proposal> {
        self.propose_contract(crate::execution::validation::Contract::Host)
    }

    fn propose_contract(
        self: &Arc<Self>,
        target: crate::execution::validation::Contract,
    ) -> Result<super::proposal::Proposal> {
        let device = self.access.device();
        let output = device
            .displays
            .iter()
            .position(|display| core::ptr::eq(&**display, self.access.display()))
            .ok_or(EINVAL)?;
        let registration = self.with_current_control(|_| {
            self.access.display().execution.propose(
                self.execution,
                &self.proposal_owner,
                target.clone(),
                || {
                    device.validation.reserve(
                        output,
                        self.access.transition_owner(),
                        self.configuration.clone(),
                        target,
                    )
                },
            )
        })?;
        Ok(super::proposal::Proposal::new(self.clone(), registration))
    }

    pub(super) fn check_proposal(
        &self,
        registration: &crate::execution::proposal::Registration,
    ) -> Result {
        self.with_current_control(|_| registration.check())
    }

    /// Recheck authority and private reservation without changing active execution.
    ///
    /// Success is an observation only. A later operation must perform its own validation
    /// and admission under the locks governing that operation.
    pub(crate) fn validate(&self) -> Result {
        self.with_current_control(|_| Ok(()))
    }

    /// Observe display metadata after checking the candidate under stable control.
    /// No lock survives the callback, and no later operation is reserved.
    fn with_current_control<R>(
        &self,
        f: impl FnOnce(display_control::Current<'_>) -> Result<R>,
    ) -> Result<R> {
        self.access
            .with_current(|current| self.with_reservation(current, f))
    }

    /// Stabilize installed display state, permission and this reservation for handoff.
    ///
    /// The callback holds master, modeset, object-ID, output, revocation and startup locks in
    /// that order. It must not wait, read pixels, acquire more modeset locks, cancel this
    /// candidate or release final resources. Only the callback may publish a control change;
    /// a returned observation does not reserve a later operation.
    pub(crate) fn with_activation_control<R>(
        &self,
        registered: &Device<Driver, Registered>,
        f: impl FnOnce(display_control::Current<'_>, &LockedState<'_, Driver>) -> Result<R>,
    ) -> Result<R> {
        self.access.with_installed(registered, |current, locked| {
            self.with_reservation(current, |current| f(current, locked))
        })
    }

    fn with_reservation<R>(
        &self,
        current: display_control::Current<'_>,
        f: impl FnOnce(display_control::Current<'_>) -> Result<R>,
    ) -> Result<R> {
        self.check_control(&current)?;
        self.resources.with_current(|| f(current))
    }

    fn check_control(&self, current: &display_control::Current<'_>) -> Result {
        if current.configuration() != &self.configuration
            || self.access.display().execution.describe() != self.execution
        {
            Err(ESTALE)
        } else {
            Ok(())
        }
    }

    /// Copy an optional image into independent private storage after checking ownership.
    ///
    /// Current authority, display configuration and reservation are checked on both sides.
    /// Copying holds no policy lock and claims no compositor source. A returned snapshot
    /// retains its historical origin; it is neither an export nor permission to activate.
    /// Descriptor delivery must separately authorize its recipient at installation.
    pub(crate) fn snapshot(&self, image: &Completed) -> Result<Snapshot> {
        self.snapshot_then(image, || Ok(()))
    }

    /// Copy the newest retained HOST result when it belongs to this control interval.
    ///
    /// No work is requested when HOST has no completed image. The retained image is only a
    /// source for fresh private storage and never escapes through the returned snapshot.
    pub(crate) fn snapshot_current(&self) -> Result<Snapshot> {
        self.validate()?;
        let host = self.access.display().host.current().map_err(|error| {
            if error == EAGAIN {
                ENODATA
            } else {
                error
            }
        })?;
        let image = host.last_image().ok_or(ENODATA)?;
        self.snapshot(&image)
    }

    /// Publish an already copied snapshot while its origin and candidate remain current.
    pub(crate) fn publish_snapshot<R>(
        &self,
        snapshot: &Snapshot,
        publish: impl FnOnce() -> R,
    ) -> Result<R> {
        self.with_current_control(|control| {
            image_access::Current::new(control)?.check_snapshot(snapshot)?;
            self.probe
                .publish_snapshot(snapshot.content_serial(), publish)
        })
    }

    /// Publish one private probe submission after checking startup on both sides.
    ///
    /// A probe reads no live compositor source. Its completion can cover an independent
    /// startup snapshot or entirely renderer-owned storage. Publication records submitted
    /// native work; it does not activate delegated execution.
    fn submit_probe(&self, source: ProbeSource, completion: Option<ARef<Fence>>) -> Result {
        self.validate()?;
        self.probe
            .submit_then(source, completion, || self.validate())
    }

    /// Publish a probe over entirely renderer-owned storage.
    pub(crate) fn submit_private_probe(&self, completion: Option<ARef<Fence>>) -> Result {
        self.submit_probe(ProbeSource::Private, completion)
    }

    /// Publish a probe that uploaded one independent HOST startup snapshot.
    pub(crate) fn submit_snapshot_probe(&self, completion: Option<ARef<Fence>>) -> Result {
        self.validate()?;
        self.probe
            .submit_snapshot_then(completion, || self.validate())
    }

    /// Inspect submitted probe completion without waiting or activating execution.
    pub(crate) fn probe_result(&self) -> Result<bool> {
        self.validate()?;
        self.probe.result()
    }

    pub(super) fn handback(
        &self,
        registered: &Device<Driver, Registered>,
        proposal: &crate::execution::proposal::Registration,
    ) -> Result<Description> {
        let device: &Device<Driver> = registered;
        if !core::ptr::eq(self.access.device(), device) {
            return Err(EINVAL);
        }
        if !matches!(
            proposal.description().profile,
            crate::execution::validation::Contract::Host
        ) {
            return Err(EINVAL);
        }
        let mut prepared = self
            .access
            .display()
            .execution
            .prepare(registered, Profile::HostV1)?;
        let description = prepared.description()?;
        // Resource lifecycle exclusion precedes display control; no drain occurs under DRM locks.
        self.access.display().host.with_change(|change| {
            self.access
                .with_installed_transition(registered, |current, locked| {
                    if self.access.display().execution.describe() != self.execution {
                        return Err(ESTALE);
                    }
                    self.resources.handback(|| {
                        proposal.check()?;
                        change.enable()?;
                        self.access.display().execution.publish_proposal(
                            locked,
                            &mut prepared,
                            proposal.description().generation,
                            current.configuration(),
                            |contract| current.check_contract(contract),
                        )
                    })
                })
        })?;
        Ok(description)
    }

    /// Publish GPU execution after the probe succeeds and a tagged update installs the gate.
    /// Metadata allocation occurs before display control. Failed publication leaves
    /// execution and candidate ownership unchanged.
    pub(super) fn activate_proposal(
        &self,
        registered: &Device<Driver, Registered>,
        proposal: &crate::execution::proposal::Registration,
    ) -> Result<(renderer_startup::Active, ProbeSource, Description)> {
        if matches!(
            proposal.description().profile,
            crate::execution::validation::Contract::Host
        ) {
            return Err(EOPNOTSUPP);
        }
        let device = self.access.device();
        let registered_device: &Device<Driver> = registered;
        if !core::ptr::eq(device, registered_device) {
            return Err(EINVAL);
        }
        let mut prepared = self
            .access
            .display()
            .execution
            .prepare(registered, Profile::GpuV1)?;
        let description = prepared.description()?;
        let (active, source) =
            self.access
                .with_installed_transition(registered, |current, locked| {
                    if self.access.display().execution.describe() != self.execution {
                        return Err(ESTALE);
                    }
                    let publish = || {
                        let source = self.probe.completed_source()?;
                        let execution = &self.access.display().execution;
                        proposal.check()?;
                        execution.publish_proposal(
                            locked,
                            &mut prepared,
                            proposal.description().generation,
                            current.configuration(),
                            |contract| current.check_contract(contract),
                        )?;
                        Ok(source)
                    };
                    self.resources.activate(publish)
                })?;
        Ok((active, source, description))
    }

    /// Claim the current live scene under this candidate's active incarnation.
    pub(crate) fn claim_source(
        &self,
        active: &renderer_startup::Active,
        execution: Description,
        previous_content_serial: Option<u64>,
    ) -> Result<SourceJob> {
        self.access.with_current(|current| {
            active.with_candidate(&self.resources, || {
                if self.access.display().execution.describe() != execution
                    || execution.profile != Profile::GpuV1
                {
                    return Err(ESTALE);
                }
                SourceJob::claim(&current, previous_content_serial, execution)
            })
        })
    }

    /// Register renderer-private backing, never storage shared with final-image recipients.
    /// Native format/modifier interpretation belongs to the trusted renderer. Registration
    /// neither maps the buffers nor proves that their native mappings exclude downstream waits.
    pub(crate) fn register_private_image(
        &self,
        active: &renderer_startup::Active,
        dimensions: [u32; 2],
        buffers: &[ARef<DmaBuf>],
    ) -> Result<Arc<Image>> {
        let check = || {
            self.access.with_current(|current| {
                active.with_candidate(&self.resources, || {
                    current.check_scene_owner()?;
                    if self.access.display().execution.describe().profile != Profile::GpuV1 {
                        return Err(ESTALE);
                    }
                    if current.configuration().dimensions() != dimensions {
                        return Err(EINVAL);
                    }
                    for buffer in buffers {
                        if current.uses_reservation(buffer.reservation())? {
                            return Err(EINVAL);
                        }
                    }
                    Ok(())
                })
            })
        };
        check()?;
        let image = Image::new(
            &self.access.device().private_images,
            &self.proposal_owner,
            dimensions,
            buffers,
        )?;
        check()?;
        Ok(image)
    }

    /// Check released source-stage evidence under live renderer authority.
    /// The callback may reserve a bounded next stage but must separately authorize capture.
    /// No lock may escape, and no pixel access or waiting is allowed in the callback.
    pub(crate) fn with_content<R>(
        &self,
        active: &renderer_startup::Active,
        content: &super::content::Released,
        f: impl FnOnce() -> Result<R>,
    ) -> Result<R> {
        self.access.with_current(|current| {
            active.with_candidate(&self.resources, || {
                content.check(&current, self.access.display().execution.describe())?;
                f()
            })
        })
    }

    fn snapshot_then(
        &self,
        image: &Completed,
        after_copy: impl FnOnce() -> Result,
    ) -> Result<Snapshot> {
        self.with_current_control(|control| {
            image_access::Current::new(control)?.check_image(image)
        })?;
        let snapshot = self.resources.snapshot(self.access.device(), image)?;
        after_copy()?;
        self.with_current_control(|control| {
            image_access::Current::new(control)?.check_snapshot(&snapshot)
        })?;
        Ok(snapshot)
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(crate) fn snapshot_then_for_test(
        &self,
        image: &Completed,
        after_copy: impl FnOnce() -> Result,
    ) -> Result<Snapshot> {
        self.snapshot_then(image, after_copy)
    }

    /// Cancel only this reservation; retained objects cannot cancel its replacement.
    pub(crate) fn cancel(&self) {
        self.resources.cancel();
        self.access
            .display()
            .execution
            .cancel_worker_proposal(&self.proposal_owner);
    }
}

impl Drop for Candidate {
    fn drop(&mut self) {
        self.cancel();
    }
}
