// SPDX-License-Identifier: GPL-2.0-only

//! Retained renderer resources with terminal revocation serialized against installation.

mod publications;

use super::{
    private_image::Image,
    private_pool::RegistrationSet,
};
use crate::execution::{
    capabilities::Profile,
    constraints::backend::Backend,
    validation::SceneView, //
};
use core::sync::atomic::{
    AtomicBool,
    Ordering, //
};
use kernel::{
    prelude::*,
    sync::{
        Arc,
        Mutex,
        MutexGuard, //
    }, //
};

/// Resources prepared for an exact output size, without modesetting or source-read authority.
///
/// Native entries may retain this identity after revocation. The endpoint owner must revoke
/// before device shutdown; draining registrations breaks resource-to-device reference cycles.
/// Already submitted jobs independently retain their own buffers and native completion.
#[pin_data]
pub(crate) struct Worker {
    output: crate::output::Identity,
    interval: crate::authority::Interval,
    profile: Profile,
    outputs: Arc<super::output_broker::Broker>,
    live: AtomicBool,
    #[pin]
    resources: Mutex<Option<Resources>>,
}

struct Resources {
    registrations: RegistrationSet,
    publications: publications::Publications,
}

/// Endpoint lifetime, distinct from retained entries and jobs. Drop outside DRM/driver locks.
#[must_use = "dropping the endpoint owner terminally revokes its worker"]
pub(crate) struct Owner {
    worker: Arc<Worker>,
    authority: kernel::sync::aref::ARef<kernel::drm::capture::Authority<Worker>>,
    permission: Option<super::permission::WorkerRegistration>,
}

// SAFETY: The module retains the worker callback and destructor through native revocation.
#[vtable]
unsafe impl kernel::drm::capture::Policy for Worker {
    fn revoke(&self) {
        Worker::revoke(self);
    }
}

impl Owner {
    /// Preparation supplies owned registrations and a successfully completed native probe.
    pub(super) fn new(
        output: crate::output::Identity,
        interval: crate::authority::Interval,
        profile: &Profile,
        dimensions: [u32; 2],
        registrations: RegistrationSet,
    ) -> Result<Self> {
        let mut limits = *profile.limits();
        if (0..2).any(|axis| {
            dimensions[axis] < limits.geometry.min_output[axis]
                || dimensions[axis] > limits.geometry.output[axis]
        }) {
            return Err(EOPNOTSUPP);
        }
        if registrations.images().next().is_none()
            || registrations
                .images()
                .any(|(_, image)| image.dimensions() != dimensions)
        {
            return Err(EINVAL);
        }
        // The allocation description must advertise the pool's actual geometry, even if
        // the renderer's declared implementation can handle a larger range of outputs.
        limits.geometry.min_output = dimensions;
        limits.geometry.output = dimensions;
        let mut formats = KVec::new();
        formats.extend_from_slice(profile.formats(), GFP_KERNEL)?;
        let profile = Profile::new(limits, formats)?;
        let publications = publications::Publications::new()?;
        let outputs = super::output_broker::Broker::new()?;
        let worker = Arc::pin_init(
            pin_init!(Worker {
                output,
                interval,
                profile,
                outputs,
                live: AtomicBool::new(true),
                resources <- kernel::new_mutex!(Some(Resources {
                    registrations,
                    publications,
                })),
            }),
            GFP_KERNEL,
        )?;
        let authority = kernel::drm::capture::Authority::new(worker.clone())?;
        Ok(Self {
            worker,
            authority,
            permission: None,
        })
    }

    pub(super) fn track_permission(&mut self, access: &super::permission::Access) -> Result {
        if self.permission.is_some() {
            return Err(EALREADY);
        }
        self.permission = Some(access.track_worker(&self.authority.revocation())?);
        Ok(())
    }

    pub(crate) fn worker(&self) -> Arc<Worker> {
        self.worker.clone()
    }

    pub(super) fn revocation(&self) -> kernel::sync::aref::ARef<kernel::drm::capture::Revocation> {
        self.authority.revocation()
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.authority.revoke();
    }
}

/// Holds resource readiness through a single native installation or authorized source claim.
/// The caller supplies authority separately and must not revoke while holding this guard.
pub(crate) struct Ready<'a> {
    worker: &'a Worker,
    resources: MutexGuard<'a, Option<Resources>>,
}

impl Worker {
    /// Advisory availability only; source admission still holds the readiness guard.
    pub(super) fn is_live(&self) -> bool {
        self.live.load(Ordering::Acquire)
    }

    pub(crate) fn output(&self) -> &crate::output::Identity {
        &self.output
    }

    pub(crate) fn interval(&self) -> crate::authority::Interval {
        self.interval
    }

    pub(crate) fn profile(&self) -> &Profile {
        &self.profile
    }

    pub(crate) fn outputs(&self) -> &Arc<super::output_broker::Broker> {
        &self.outputs
    }

    /// Advisory validation, safe inside a callback while installation holds the Ready guard.
    /// It must not recursively acquire the resource mutex. Final acceptance holds that guard
    /// through state swap; otherwise this observation cannot exclude concurrent revocation.
    pub(crate) fn check_scene(&self, scene: SceneView<'_>) -> Result {
        if !self.live.load(Ordering::Acquire) {
            return Err(EKEYREVOKED);
        }
        match scene {
            SceneView::Disabled => Ok(()),
            SceneView::Enabled { scene, output } => self.profile.check(scene, output),
        }
    }

    pub(crate) fn hold_ready(&self) -> Result<Ready<'_>> {
        let resources = self.resources.lock();
        if resources.is_none() {
            return Err(EKEYREVOKED);
        }
        Ok(Ready { worker: self, resources })
    }

    /// Compose several independent output guards without waiting while another is held.
    /// A busy cohort rejects acceptance and must be retried as a complete transaction.
    pub(crate) fn try_hold_ready(&self) -> Result<Ready<'_>> {
        let resources = self.resources.try_lock().ok_or(EBUSY)?;
        if resources.is_none() {
            return Err(EKEYREVOKED);
        }
        Ok(Ready { worker: self, resources })
    }

    /// Exclude installation, mark terminal, then withdraw offers and release private storage.
    /// Call outside native DRM and provider locks; no native fence is signaled here.
    pub(crate) fn revoke(&self) {
        let retired = {
            let mut resources = self.resources.lock();
            self.live.store(false, Ordering::Release);
            resources.take()
        };
        if let Some(Resources { registrations, publications }) = retired {
            self.outputs.close();
            publications.withdraw();
            drop(registrations);
        }
    }
}

impl Ready<'_> {
    /// The held readiness exclusion belongs to this exact retained worker.
    pub(super) fn belongs_to(&self, worker: &Worker) -> bool {
        core::ptr::eq(self.worker, worker)
    }

    /// Publish with a retained withdrawal record while endpoint revocation is excluded.
    pub(crate) fn publish(
        &mut self,
        output: &kernel::drm::kms::constraints::Output<'_, crate::Driver>,
        entry: &kernel::drm::constraints::Entry<Backend>,
    ) -> Result {
        let backend = entry.backend();
        let Backend::Renderer(worker) = &*backend else {
            return Err(EOPNOTSUPP);
        };
        if !self.belongs_to(worker) {
            return Err(EACCES);
        }
        (*self.resources).as_mut().ok_or(EKEYREVOKED)?.publications.publish(output, entry)
    }

    /// Exact registered destination identity, not permission to read or write it.
    pub(crate) fn contains(&self, id: u64, image: &Image) -> bool {
        self.resources.as_ref().is_some_and(|resources| {
            resources.registrations.images()
                .any(|(name, stored)| name == id && core::ptr::eq(stored, image))
        })
    }
}
