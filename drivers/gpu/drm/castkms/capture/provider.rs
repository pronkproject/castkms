// SPDX-License-Identifier: GPL-2.0-only

//! Kernel-issued capture of independently completed host images.
//!
//! Native authority and raw streams remain private. The only delivery method validates
//! current display access and image ownership before claiming a job, then copies outside
//! policy locks. No operation exports source buffers or represents asynchronous GPU access.

mod request;
mod storage;

pub(crate) use request::Request;

use super::{
    host,
    permission::Permission,
    streams::Registration, //
};
use crate::{
    host_compositor::compose::Completed,
    scene::Configuration, //
};
use kernel::{
    drm::capture::{
        Authority,
        Policy as NativePolicy,
        Stream as NativeStream, //
    },
    prelude::*,
    sync::{
        aref::ARef,
        Arc, //
    }, //
};
use storage::Storage;

struct Policy {
    permission: Permission,
}

// SAFETY: The vtable selects CastKMS's module, retaining the callbacks and private policy code.
#[vtable]
unsafe impl NativePolicy for Policy {
    fn revoke(&self) {
        // Native revocation ends registered delivery. Private jobs and results retain their
        // own storage; the provider has no asynchronous source access to cancel or wait for.
    }

    fn authorize_capture(&self, _: &NativeStream) -> Result {
        // Stream::deliver is the only authority claim site. It validates the stream interval
        // and completed image under Permission::with_current before entering native admission,
        // and retains those outer locks across this callback and the native claim.
        Ok(())
    }
}

/// Unique revocation owner for a kernel-issued grant, independent of its capture handles.
#[must_use = "dropping the grantor revokes capture"]
pub(crate) struct Grantor {
    capture: Capture,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Grantor {
    /// Create a kernel grant from an established target, without impersonating a DRM file.
    pub(crate) fn new(permission: Permission) -> Result<Self> {
        let policy = Arc::new(Policy { permission }, GFP_KERNEL)?;
        let authority = Authority::new(policy.clone())?;
        Ok(Self {
            capture: Capture { authority, policy },
        })
    }

    pub(crate) fn capture(&self) -> Capture {
        self.capture.clone()
    }
}

impl Drop for Grantor {
    fn drop(&mut self) {
        self.capture.authority.revoke();
    }
}

/// Capture capability without the grantor's revocation ownership.
#[derive(Clone)]
pub(crate) struct Capture {
    authority: ARef<Authority<Policy>>,
    policy: Arc<Policy>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Capture {
    /// Allocate outside policy locks, then revalidate the same configuration at registration.
    pub(crate) fn stream(&self, capacity: u32) -> Result<Stream> {
        if self.authority.is_revoked() {
            return Err(EKEYREVOKED);
        }
        let permission = &self.policy.permission;
        let (configuration, layout) = permission
            .with_current(|current| Ok((current.configuration().clone(), current.layout())))?;
        let charge = permission
            .device()
            .capture_budget
            .reserve(layout, capacity)?;
        let storage = Storage::new(charge, self.authority.clone())?;
        let registered = permission.with_current(|current| {
            if current.configuration() != &configuration {
                return Err(ESTALE);
            }
            let admission = self.authority.begin()?;
            admission.add_stream(&storage.native)?;
            permission
                .device()
                .capture_streams
                .register(&storage.native, current.configuration())
        });
        let registration = match registered {
            Ok(registration) => registration,
            Err(error) => {
                // Roll back outside every policy lock. The local storage owner keeps native
                // stream destruction and its budget release outside the admission callback.
                self.authority.remove_stream(&storage.native);
                return Err(error);
            }
        };
        Ok(Stream {
            _registration: registration,
            storage,
            capture: self.clone(),
            configuration,
        })
    }
}

/// One configuration's delivery, independently owned from source reads and private composition.
///
/// Closing the stream abandons its requests, including completed results. Revoking the grantor
/// instead stops new delivery while retaining already completed, authorized results.
pub(crate) struct Stream {
    _registration: Registration,
    storage: Arc<Storage>,
    capture: Capture,
    configuration: Configuration,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Stream {
    /// Recheck an allocated stream before arranging additional resources.
    ///
    /// The result is an observation, not continuing pixel permission. Delivery still
    /// checks the image and claims through the native authority under policy locks.
    pub(crate) fn check_current(&self) -> Result {
        self.capture.policy.permission.with_current(|current| {
            if current.configuration() != &self.configuration {
                return Err(ESTALE);
            }
            let _admission = self.capture.authority.begin()?;
            Ok(())
        })
    }

    pub(crate) fn queue(&self) -> Result<Request> {
        Request::new(&self.storage)
    }

    /// Claim authorized delivery before copying, without retaining a compositor source.
    ///
    /// Success means one job was claimed and completed; its retained result reports any
    /// copy failure or intervening revocation. Denial leaves queued demand unchanged.
    pub(crate) fn deliver(&self, image: &Completed) -> Result {
        let job = self.capture.policy.permission.with_current(|current| {
            if current.configuration() != &self.configuration {
                return Err(ESTALE);
            }
            current.check_image(image)?;
            self.capture.authority.claim(&self.storage.native)
        })?;
        host::complete(image, job);
        Ok(())
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // Close has the same result-discard semantics even if revocation removed membership.
        self.storage.native.shutdown();
        self.capture.authority.remove_stream(&self.storage.native);
    }
}
