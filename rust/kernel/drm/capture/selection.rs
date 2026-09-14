// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Selection that preserves a request's stream identity instead of choosing queued demand.

use super::{
    Job,
    Request, //
};
use crate::{
    error::from_err_ptr,
    prelude::*, //
};
use core::ptr::NonNull;

impl Request {
    /// Claim exactly this request for synchronous kernel-controlled pixel production.
    ///
    /// This does not check pixel permission or authority membership. An authority-managed
    /// provider must use its authority's claim operation instead. The caller stabilizes
    /// any source/policy state across the claim. No asynchronous access is represented.
    /// A claimed or completed request returns `EALREADY`; no substitute is ever selected.
    pub fn claim(&self) -> Result<Job> {
        // SAFETY: The request retains its originating stream and immutable native ID.
        let raw = from_err_ptr(unsafe {
            bindings::drm_capture_claim_request(self.stream.0.get(), self.id)
        })?;
        Ok(Job {
            // SAFETY: Native claim transfers one non-null, exclusively owned job.
            ptr: unsafe { NonNull::new_unchecked(raw) },
        })
    }
}

#[cfg(CONFIG_KUNIT)]
#[kunit_tests(rust_drm_capture_selection)]
mod tests {
    use super::*;
    use crate::drm::capture::{
        Status,
        Stream, //
    };

    #[test]
    fn reversed_completion_preserves_each_result() -> Result {
        let stream = Stream::new(2, 4)?;
        let first = stream.queue()?;
        let second = stream.queue()?;
        let mut job = second.claim()?;
        job.data_mut().copy_from_slice(&[2; 4]);
        job.complete(Ok(()));
        assert_eq!(first.status()?, Status::Pending);
        let mut job = first.claim()?;
        job.data_mut().copy_from_slice(&[1; 4]);
        job.complete(Ok(()));
        let mut pixels = [0; 4];
        first.copy_result(&mut pixels)?;
        assert_eq!(pixels, [1; 4]);
        second.copy_result(&mut pixels)?;
        assert_eq!(pixels, [2; 4]);
        Ok(())
    }

    #[test]
    fn canceled_and_claimed_requests_do_not_select_another() -> Result {
        let stream = Stream::new(2, 4)?;
        let first = stream.queue()?;
        let second = stream.queue()?;
        first.cancel()?;
        assert!(matches!(first.claim(), Err(EALREADY)));
        assert_eq!(second.status()?, Status::Pending);
        let job = second.claim()?;
        assert!(matches!(second.claim(), Err(EALREADY)));
        drop(second);
        assert!(matches!(stream.queue(), Err(EAGAIN)));
        job.complete(Ok(()));
        let _next = stream.queue()?;
        Ok(())
    }
}
