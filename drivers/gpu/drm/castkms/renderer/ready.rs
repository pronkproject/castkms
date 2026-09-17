// SPDX-License-Identifier: GPL-2.0-only

//! Retained renderer resources with terminal revocation serialized against installation.

use super::{
    private_image::Image,
    private_pool::RegistrationSet,
    probe::Source, //
};
use crate::execution::{
    capabilities::Profile,
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
    source: Source,
    live: AtomicBool,
    #[pin]
    registrations: Mutex<Option<RegistrationSet>>,
}

/// Endpoint lifetime, distinct from retained entries and jobs. Drop outside DRM/driver locks.
#[must_use = "dropping the endpoint owner terminally revokes its worker"]
pub(crate) struct Owner {
    worker: Arc<Worker>,
    authority: kernel::sync::aref::ARef<kernel::drm::capture::Authority<Worker>>,
    permission: Option<crate::authority::grants::Registration>,
}

// SAFETY: The module retains the worker callback and destructor through native revocation.
#[vtable]
unsafe impl kernel::drm::capture::Policy for Worker {
    fn revoke(&self) {
        Worker::revoke(self);
    }
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Owner {
    /// The candidate supplies owned registrations and a successfully completed native probe.
    pub(super) fn new(
        output: crate::output::Identity,
        interval: crate::authority::Interval,
        profile: &Profile,
        dimensions: [u32; 2],
        registrations: RegistrationSet,
        source: Source,
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
        let worker = Arc::pin_init(
            pin_init!(Worker {
                output,
                interval,
                profile,
                source,
                live: AtomicBool::new(true),
                registrations <- kernel::new_mutex!(Some(registrations)),
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
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.authority.revoke();
    }
}

/// Holds resource readiness through a single native installation or authorized source claim.
/// The caller supplies authority separately and must not revoke while holding this guard.
pub(crate) struct Ready<'a> {
    registrations: MutexGuard<'a, Option<RegistrationSet>>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Worker {
    pub(crate) fn output(&self) -> &crate::output::Identity {
        &self.output
    }

    pub(crate) fn interval(&self) -> crate::authority::Interval {
        self.interval
    }

    pub(crate) fn profile(&self) -> &Profile {
        &self.profile
    }

    pub(crate) fn probe_source(&self) -> Source {
        self.source
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
        let registrations = self.registrations.lock();
        if registrations.is_none() {
            return Err(EKEYREVOKED);
        }
        Ok(Ready { registrations })
    }

    /// Exclude installation, mark terminal, then drop storage outside the readiness lock.
    /// Call outside native DRM and provider locks; no native fence is signaled here.
    pub(crate) fn revoke(&self) {
        let retired = {
            let mut registrations = self.registrations.lock();
            self.live.store(false, Ordering::Release);
            registrations.take()
        };
        drop(retired);
    }
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Ready<'_> {
    /// Exact registered destination identity, not permission to read or write it.
    pub(crate) fn contains(&self, id: u64, image: &Image) -> bool {
        self.registrations.as_ref().is_some_and(|set| {
            set.images()
                .any(|(name, stored)| name == id && core::ptr::eq(stored, image))
        })
    }
}
