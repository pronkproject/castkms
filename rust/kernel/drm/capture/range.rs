// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Bounded copies of completed pixels without exposing their private storage.

use super::Request;
use crate::prelude::*;

impl Request {
    /// Copy an exact byte range without acknowledging or reserving the result.
    ///
    /// Pending, failed, missing and out-of-range results leave output unchanged. The
    /// complete range must fit; there are no short successful copies. A zero-length
    /// range at the image end is valid but still checks result validity. Concurrent
    /// shutdown may discard the result between successive calls.
    pub fn copy_result_range(&self, offset: usize, output: &mut [u8]) -> Result {
        // SAFETY: This request retains its originating stream. The slice describes all
        // writable output storage; native code validates the exact source range.
        let result = unsafe {
            bindings::drm_capture_copy_result_range(
                self.stream.0.get(),
                self.id,
                offset,
                output.as_mut_ptr().cast(),
                output.len(),
            )
        };
        if result < 0 {
            Err(Error::from_errno(result as i32))
        } else {
            Ok(())
        }
    }
}

#[cfg(CONFIG_KUNIT)]
mod tests {
    use super::*;
    use crate::drm::capture::Stream;

    #[kunit_tests(rust_drm_capture_ranges)]
    mod cases {
        use super::*;

        #[test]
        fn ranges_retain_their_originating_request() -> Result {
            let stream = Stream::new(1, 16)?;
            let request = stream.queue()?;
            let mut job = stream.claim()?;
            for (index, pixel) in job.data_mut().iter_mut().enumerate() {
                *pixel = index as u8;
            }
            job.complete(Ok(()));
            drop(stream);
            let mut output = [0; 4];
            request.copy_result_range(7, &mut output)?;
            if output != [7, 8, 9, 10] {
                return Err(EINVAL);
            }
            request.copy_result_range(16, &mut [])?;
            if request.copy_result_range(usize::MAX, &mut output) != Err(EINVAL)
                || output != [7, 8, 9, 10]
            {
                return Err(EINVAL);
            }
            Ok(())
        }

        #[test]
        fn pending_and_failed_ranges_do_not_expose_storage() -> Result {
            let stream = Stream::new(1, 16)?;
            let request = stream.queue()?;
            let mut output = [0x19; 4];
            if request.copy_result_range(0, &mut output) != Err(EAGAIN) {
                return Err(EINVAL);
            }
            stream.claim()?.complete(Err(EIO));
            if request.copy_result_range(0, &mut output) != Err(EIO) || output != [0x19; 4] {
                return Err(EINVAL);
            }
            stream.shutdown();
            if request.copy_result_range(0, &mut output) != Err(ENOENT) {
                return Err(EINVAL);
            }
            Ok(())
        }
    }
}
