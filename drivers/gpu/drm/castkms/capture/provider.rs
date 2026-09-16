// SPDX-License-Identifier: GPL-2.0-only

//! Kernel-issued capture of independently completed host images.
//!
//! Native authority and raw streams remain private. Each delivery operation validates
//! current display access and image ownership before claiming a job, then copies outside
//! policy locks. No operation exports source buffers or represents asynchronous GPU access.

mod creator;
mod control_file;
mod client_file;
mod files;
mod description;
mod delegated;
mod delegated_destination;
mod frame;
mod request;
mod storage;

pub(crate) use creator::Creator;
pub(crate) use description::Description;
pub(crate) use frame::Frame;
pub(crate) use request::Request;

use super::{
    host,
    permission::Permission,
    streams::Registration, //
};
use crate::{
    authority::grants,
    host_compositor::{
        compose::Completed,
        layout::Layout, //
    },
    scene::Configuration,
    Driver, //
};
use kernel::{
    dma_buf::DmaBuf,
    drm::{
        capture::{
            Authority,
            Policy as NativePolicy,
            Stream as NativeStream, //
        },
        Device, //
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
        // Both delivery operations validate the stream interval and completed image through
        // Stream::with_image before entering native admission. They retain the outer policy
        // locks across this callback and the native claim.
        Ok(())
    }
}

/// Unique revocation owner for a kernel-issued grant, independent of its capture handles.
#[must_use = "dropping the grantor revokes capture"]
pub(crate) struct Grantor {
    capture: Capture,
    creator: Option<creator::Registration>,
    _device_registration: grants::Registration,
}

impl Grantor {
    /// Create a kernel grant from an established target, without impersonating a DRM file.
    pub(crate) fn new(permission: Permission) -> Result<Self> {
        let policy = Arc::new(Policy { permission }, GFP_KERNEL)?;
        let authority = Authority::new(policy.clone())?;
        let device_registration = policy
            .permission
            .device()
            .capture_grants
            .register(&authority.revocation())?;
        Ok(Self {
            capture: Capture { authority, policy },
            creator: None,
            _device_registration: device_registration,
        })
    }

    /// Attach one external lifetime without transferring the grantor's own revocation duty.
    ///
    /// The caller must keep issuance authority stable until registration returns. On failure,
    /// the existing grantor remains owned by the caller and must be dropped outside DRM locks.
    pub(crate) fn track_creator(&mut self, creator: &Creator) -> Result {
        if self.creator.is_some() {
            return Err(EEXIST);
        }
        self.creator = Some(creator.register(&self.capture.authority)?);
        Ok(())
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
    /// Describe and open the current stream layout in one kernel convenience operation.
    pub(crate) fn stream(&self, capacity: u32) -> Result<Stream> {
        self.describe_stream()?.create_stream(capacity)
    }

    /// Allocate outside policy locks, checking the described configuration on both sides.
    fn create_stream(
        &self,
        configuration: &Configuration,
        layout: Layout,
        capacity: u32,
    ) -> Result<Stream> {
        if self.authority.is_revoked() {
            return Err(EKEYREVOKED);
        }
        let permission = &self.policy.permission;
        permission.with_current(|current| {
            permission.display().execution.check_host()?;
            if current.configuration() != configuration {
                return Err(ESTALE);
            }
            Ok(())
        })?;
        let charge = permission
            .device()
            .capture_budget
            .reserve(layout, capacity)?;
        let storage = Storage::new(charge, self.authority.clone())?;
        let registered = permission.with_current(|current| {
            permission.display().execution.check_host()?;
            if current.configuration() != configuration {
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
            configuration: configuration.clone(),
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
    /// Borrow the allocation device without granting access to its current pixels.
    pub(super) fn device(&self) -> &Device<Driver> {
        self.capture.policy.permission.device()
    }

    pub(super) fn host(&self) -> &crate::host_compositor::configuration::Configuration {
        &self.capture.policy.permission.display().host
    }

    pub(super) fn layout(&self) -> Layout {
        self.storage.layout()
    }

    /// Recheck an allocated stream before arranging additional resources.
    ///
    /// The result is an observation, not continuing pixel permission. Delivery still
    /// checks the image and claims through the native authority under policy locks.
    pub(crate) fn check_current(&self) -> Result {
        self.capture.policy.permission.with_current(|current| {
            self.capture
                .policy
                .permission
                .display()
                .execution
                .check_host()?;
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

    /// Reject a known source alias before admitting destination work.
    ///
    /// The caller must still exclude competing destination users throughout delivery.
    /// No authority, source or storage reservation escapes this metadata observation.
    pub(super) fn check_destination(&self, buffer: &DmaBuf) -> Result {
        self.capture.policy.permission.with_current(|current| {
            if current.configuration() != &self.configuration {
                return Err(ESTALE);
            }
            let _admission = self.capture.authority.begin()?;
            current.check_destination(buffer)
        })
    }

    /// Claim authorized delivery before copying, without retaining a compositor source.
    ///
    /// Success means one job was claimed and completed; its retained result reports any
    /// copy failure or intervening revocation. Denial leaves queued demand unchanged.
    pub(crate) fn deliver(&self, image: &Completed) -> Result {
        let job = self.with_image(image, || self.capture.authority.claim(&self.storage.native))?;
        host::complete(image, job);
        Ok(())
    }

    fn with_image<R>(&self, image: &Completed, f: impl FnOnce() -> Result<R>) -> Result<R> {
        self.capture.policy.permission.with_current(|current| {
            if current.configuration() != &self.configuration {
                return Err(ESTALE);
            }
            current.check_image(image)?;
            f()
        })
    }

    /// End delivery even when asynchronous operations retain the stream's storage owner.
    pub(crate) fn close(&self) {
        // Close discards results even if revocation already removed membership.
        self.storage.native.shutdown();
        self.capture.authority.remove_stream(&self.storage.native);
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.close();
    }
}
