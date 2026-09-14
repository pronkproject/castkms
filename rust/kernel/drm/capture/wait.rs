// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Interruptible result observation without file transport or provider policy.

use super::Request;
use crate::{
    error::to_result,
    prelude::*,
    sync::CondVar, //
};

impl Request {
    /// Observe provider readiness without postponing a terminal capture error.
    ///
    /// The callback may sleep and runs without native capture or waitqueue locks held.
    /// Notify `changed` whenever its result may change. Both that queue and the capture
    /// result queue are registered before observation, including the check-to-sleep gap.
    /// Call outside locks needed by the provider; do not recursively enter this helper.
    /// An already successful request returns `EALREADY`. Readiness neither claims a job
    /// nor preserves permission; delivery must still perform its ordinary authorization.
    pub fn wait_for_provider<T, F>(&self, changed: &CondVar, observe: F) -> Result<T>
    where
        F: FnMut() -> Result<Option<T>>,
    {
        struct Observation<T, F> {
            observe: F,
            value: Option<T>,
        }

        unsafe extern "C" fn ready<T, F>(data: *mut crate::ffi::c_void) -> i32
        where
            F: FnMut() -> Result<Option<T>>,
        {
            // SAFETY: The synchronous native wait borrows the exclusive stack context.
            // Only this callback accesses it until that wait returns; it is not retained.
            let observation = unsafe { &mut *data.cast::<Observation<T, F>>() };
            match (observation.observe)() {
                Ok(Some(value)) => {
                    observation.value = Some(value);
                    1
                }
                Ok(None) => 0,
                Err(error) => error.to_errno(),
            }
        }

        let mut observation = Observation {
            observe,
            value: None,
        };
        // SAFETY: Both queues remain live through their borrows. The native helper invokes
        // the exact typed callback synchronously and removes every waiter before returning.
        to_result(unsafe {
            bindings::drm_capture_wait_provider(
                self.stream.0.get(),
                self.id,
                changed.wait_queue_head.get(),
                Some(ready::<T, F>),
                (&mut observation as *mut Observation<T, F>).cast(),
            )
        })?;
        observation.value.ok_or(EIO)
    }

    /// Wait for a completed result without consuming the request or its image.
    ///
    /// The outer result describes waiting: interruption or removal of the request is an error.
    /// The inner result describes capture: provider failure is a completed, unsuccessful image.
    /// Interruption does not cancel the request. Revoking or canceling a claimed request does
    /// not finish the wait until its provider ends access. Hold no locks needed by that provider.
    pub fn wait(&self) -> Result<Result> {
        let mut result = bindings::drm_capture_result::default();
        // SAFETY: The request retains its stream and output storage remains writable until return.
        to_result(unsafe {
            bindings::drm_capture_wait_result(self.stream.0.get(), self.id, &mut result)
        })?;
        Ok(to_result(result.status))
    }
}

#[cfg(CONFIG_KUNIT)]
mod tests;
