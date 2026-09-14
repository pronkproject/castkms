// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Synchronous result publication without lending a callback beyond its call.

use super::ClientStream;
use crate::{
    drm::capture::Completion,
    error::to_result,
    prelude::*, //
};
use core::ffi::c_void;

struct Publication<F, R> {
    publish: Option<F>,
    result: Option<Result<R>>,
}

unsafe extern "C" fn publish<F, R>(
    data: *mut c_void,
    raw: *const bindings::drm_capture_completion,
) -> i32
where
    F: FnOnce(Completion) -> Result<R>,
{
    // SAFETY: Dequeue supplies this stack-owned publication exclusively for the synchronous
    // native call. Neither native dispatch nor its provider retains the sink or context.
    let publication = unsafe { &mut *data.cast::<Publication<F, R>>() };
    let Some(publish) = publication.publish.take() else {
        return EALREADY.to_errno();
    };
    // SAFETY: Native dispatch validates and borrows completion metadata for this callback.
    let result = Completion::from_raw(unsafe { &*raw }).and_then(publish);
    let status = result.as_ref().map_or_else(|error| error.to_errno(), |_| 0);
    publication.result = Some(result);
    status
}

impl ClientStream {
    /// Publish one terminal record and acknowledge it only if the closure succeeds.
    ///
    /// A capture error is carried inside Completion. EAGAIN without invoking the closure
    /// means no terminal record is available. Closure errors are returned unchanged and
    /// leave that same record queued, including when the closure itself returns EAGAIN.
    /// The closure may borrow local state and return an owned value. It must not reenter
    /// this client, and no closure borrow escapes the synchronous native operation.
    pub fn dequeue<R>(&self, publish_result: impl FnOnce(Completion) -> Result<R>) -> Result<R> {
        fn invoke<F, R>(client: &ClientStream, callback: F) -> Result<R>
        where
            F: FnOnce(Completion) -> Result<R>,
        {
            let id = client.id.ok_or(ENOENT)?;
            let mut publication = Publication {
                publish: Some(callback),
                result: None,
            };
            let sink = bindings::drm_capture_completion_sink {
                publish: Some(publish::<F, R>),
                data: core::ptr::from_mut(&mut publication).cast(),
            };
            // SAFETY: The client file, sink and uniquely borrowed context remain live until
            // synchronous native dispatch returns. The callback consumes its FnOnce at most
            // once and saves any owned return value without exposing a borrow to native code.
            to_result(unsafe {
                bindings::drm_capture_client_dequeue(client.client.as_ptr(), id.get(), &sink)
            })?;
            publication.result.ok_or(EIO)?
        }

        invoke(self, publish_result)
    }
}

#[cfg(CONFIG_KUNIT)]
mod tests;
