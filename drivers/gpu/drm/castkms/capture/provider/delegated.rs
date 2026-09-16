// SPDX-License-Identifier: GPL-2.0-only

//! Capture scope for delegated images, independent of the HOST compositor's layout.

use super::Capture;
use crate::{
    display_control::Current,
    execution::{Description, Profile},
    renderer::{candidate::Candidate, render_job::Rendered},
    renderer_startup::Observation,
    scene::Configuration,
};
use kernel::prelude::*;

/// Exact grant and display interval, without storage, source reads or renderer ownership.
#[derive(Clone)]
pub(crate) struct Delegated {
    capture: Capture,
    configuration: Configuration,
    execution: Description,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Capture {
    /// Describe delegated output under capture authority, without granting raw source access.
    pub(crate) fn describe_delegated(&self) -> Result<Delegated> {
        let permission = &self.policy.permission;
        permission.with_control(|current| {
            let _admission = self.authority.begin()?;
            let execution = permission.display().execution.describe();
            if execution.profile != Profile::GpuV1 {
                return Err(EOPNOTSUPP);
            }
            Ok(Delegated {
                capture: self.clone(),
                configuration: current.configuration().clone(),
                execution,
            })
        })
    }
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Delegated {
    /// Publish demand to the discovered worker without transferring capture ownership.
    pub(crate) fn register_routed_queue(
        &self,
        capacity: u32,
    ) -> Result<crate::renderer::output_broker::Registration> {
        self.capture
            .policy
            .permission
            .display()
            .renderer_routes
            .lookup()?
            .register_queue(self, capacity)
    }

    /// Discover the current output worker, then intersect both authorities at admission.
    pub(crate) fn create_routed_queue(
        &self,
        capacity: u32,
    ) -> Result<super::delegated_queue::Queue> {
        self.capture
            .policy
            .permission
            .display()
            .renderer_routes
            .lookup()?
            .create_queue(self, capacity)
    }

    /// Shared advisory notifications do not identify a grant or authorize any operation.
    pub(super) fn changed(&self) -> kernel::sync::Arc<kernel::sync::poll::PollCondVar> {
        self.capture.policy.permission.device().changed.clone()
    }

    pub(super) fn request_budget(
        &self,
    ) -> &kernel::sync::Arc<crate::capture::request_budget::Budget> {
        &self
            .capture
            .policy
            .permission
            .device()
            .capture_request_budget
    }

    pub(super) fn check_same(&self, other: &Self) -> Result {
        if !core::ptr::eq(&*self.capture.authority, &*other.capture.authority) {
            return Err(EACCES);
        }
        if self.configuration != other.configuration || self.execution != other.execution {
            return Err(ESTALE);
        }
        Ok(())
    }

    /// Validate the worker and recipient under one display guard without selecting pixels.
    pub(super) fn with_renderer<R>(
        &self,
        renderer: &Candidate,
        active: &Observation,
        f: impl FnOnce(&Current<'_>) -> Result<R>,
    ) -> Result<R> {
        renderer.with_observed_control(active, |current| {
            self.check(current)?;
            let _admission = self.capture.authority.begin()?;
            f(current)
        })
    }

    pub(super) fn storage_registry(&self) -> &kernel::sync::Arc<crate::image_storage::Registry> {
        &self.capture.policy.permission.device().image_storage
    }

    pub(crate) fn dimensions(&self) -> [u32; 2] {
        self.configuration.dimensions()
    }

    fn check(&self, current: &Current<'_>) -> Result {
        let permission = &self.capture.policy.permission;
        permission.check_control(current)?;
        if current.configuration() != &self.configuration
            || permission.display().execution.describe() != self.execution
        {
            return Err(ESTALE);
        }
        Ok(())
    }

    /// Stabilize current capture admission without claiming storage or a compositor source.
    pub(crate) fn with_current<R>(&self, f: impl FnOnce(&Current<'_>) -> Result<R>) -> Result<R> {
        self.capture.policy.permission.with_control(|current| {
            self.check(&current)?;
            let _admission = self.capture.authority.begin()?;
            f(&current)
        })
    }

    /// Require both renderer and recipient authority at one bounded output-stage claim.
    /// The callback holds display, renderer and capture-admission locks. It must not access
    /// pixels, wait, recurse into capabilities or release retained DRM references.
    pub(crate) fn with_image<R>(
        &self,
        renderer: &Candidate,
        active: &Observation,
        image: &Rendered,
        f: impl FnOnce(&Current<'_>) -> Result<R>,
    ) -> Result<R> {
        renderer.with_observed_content(active, image.content(), |current| {
            self.check(current)?;
            let _admission = self.capture.authority.begin()?;
            f(current)
        })
    }
}
