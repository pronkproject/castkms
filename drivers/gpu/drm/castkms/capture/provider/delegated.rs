// SPDX-License-Identifier: GPL-2.0-only

//! Capture scope for output from one exact accepted renderer worker.

use super::Capture;
use crate::{
    display_control::Current,
    renderer::{ready::Worker, render_job::Rendered},
    scene::Configuration,
};
use kernel::{prelude::*, sync::Arc};

/// Exact grant, configuration and accepted worker, without pixel access.
#[derive(Clone)]
pub(crate) struct Delegated {
    capture: Capture,
    configuration: Configuration,
    worker: Arc<Worker>,
}

impl Capture {
    pub(crate) fn describe_delegated(&self) -> Result<Delegated> {
        let permission = &self.policy.permission;
        permission.with_control(|current| {
            let _admission = self.authority.begin()?;
            let worker = current.renderer_worker()?;
            drop(worker.hold_ready()?);
            Ok(Delegated {
                capture: self.clone(),
                configuration: current.configuration().clone(),
                worker,
            })
        })
    }
}

impl Delegated {
    pub(crate) fn register_queue(
        &self,
        capacity: u32,
    ) -> Result<crate::renderer::output_broker::Registration> {
        self.with_worker(|_| Ok(()))?;
        self.worker.outputs().register(|| self.create_queue(capacity))
    }

    pub(crate) fn create_queue(&self, capacity: u32) -> Result<super::delegated_queue::Queue> {
        super::delegated_queue::Queue::new(self, capacity)
    }

    pub(super) fn changed(&self) -> Arc<kernel::sync::poll::PollCondVar> {
        self.capture.policy.permission.device().changed.clone()
    }

    pub(super) fn request_budget(&self) -> &Arc<crate::capture::request_budget::Budget> {
        &self.capture.policy.permission.device().capture_request_budget
    }

    pub(super) fn check_same(&self, other: &Self) -> Result {
        if !core::ptr::eq(&*self.capture.authority, &*other.capture.authority) {
            return Err(EACCES);
        }
        if self.configuration != other.configuration || !Arc::ptr_eq(&self.worker, &other.worker) {
            return Err(ESTALE);
        }
        Ok(())
    }

    pub(crate) fn same_stream(&self, other: &Self) -> bool {
        core::ptr::eq(&*self.capture.authority, &*other.capture.authority)
            && self.configuration == other.configuration
            && Arc::ptr_eq(&self.worker, &other.worker)
    }

    pub(super) fn storage_registry(&self) -> &Arc<crate::image_storage::Registry> {
        &self.capture.policy.permission.device().image_storage
    }

    pub(crate) fn dimensions(&self) -> [u32; 2] {
        self.configuration.dimensions()
    }

    pub(crate) fn configuration(&self) -> &Configuration {
        &self.configuration
    }

    fn check(&self, current: &Current<'_>) -> Result {
        let permission = &self.capture.policy.permission;
        permission.check_control(current)?;
        if current.configuration() != &self.configuration
            || !Arc::ptr_eq(&current.renderer_worker()?, &self.worker)
        {
            return Err(ESTALE);
        }
        Ok(())
    }

    pub(crate) fn with_current<R>(&self, f: impl FnOnce(&Current<'_>) -> Result<R>) -> Result<R> {
        self.capture.policy.permission.with_control(|current| {
            self.check(&current)?;
            let _admission = self.capture.authority.begin()?;
            f(&current)
        })
    }

    pub(super) fn with_worker<R>(&self, f: impl FnOnce(&Current<'_>) -> Result<R>) -> Result<R> {
        self.with_current(|current| {
            let _ready = self.worker.hold_ready()?;
            f(current)
        })
    }

    pub(crate) fn with_image<R>(
        &self,
        image: &Rendered,
        f: impl FnOnce(&Current<'_>) -> Result<R>,
    ) -> Result<R> {
        self.with_worker(|current| {
            image.content().check_bound(current)?;
            f(current)
        })
    }
}
