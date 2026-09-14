// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Terminal delivery metadata without image storage or publication ownership.

use crate::{
    prelude::*,
    time::{
        Instant,
        Monotonic, //
    }, //
};
use core::num::NonZeroU64;

/// A completed attempt whose destination writes have ended.
///
/// Success describes a valid image with its original monotonic production time. Failure
/// carries an error instead of an image timestamp. Neither outcome acknowledges a queue
/// record or grants pixel access; the provider enforces those separate contracts.
#[derive(Clone, Copy)]
pub struct Completion {
    use_id: NonZeroU64,
    result: Result<Instant<Monotonic>>,
}

impl Completion {
    /// Describe a terminal attempt under its nonzero stream-local name.
    ///
    /// The provider must finish all destination access before publishing this value.
    /// Image production time is not presentation or dequeue time; cached images may
    /// repeat it, and out-of-order results need not have increasing timestamps.
    pub fn new(use_id: u64, result: Result<Instant<Monotonic>>) -> Result<Self> {
        Ok(Self {
            use_id: NonZeroU64::new(use_id).ok_or(EINVAL)?,
            result,
        })
    }

    /// Request name within the stream being dequeued, not an allocation or authority ID.
    pub fn use_id(&self) -> u64 {
        self.use_id.get()
    }

    /// Image production time on success, or the terminal capture error.
    pub fn result(&self) -> Result<Instant<Monotonic>> {
        self.result
    }

    pub(super) fn raw(self) -> bindings::drm_capture_completion {
        let (status, completed_at) = match self.result {
            Ok(time) => (0, time.as_nanos()),
            Err(error) => (error.to_errno(), 0),
        };
        bindings::drm_capture_completion {
            use_id: self.use_id(),
            status,
            completed_at,
        }
    }

    pub(super) fn from_raw(raw: &bindings::drm_capture_completion) -> Result<Self> {
        if raw.completed_at < 0 || raw.status > 0 || raw.status < -(bindings::MAX_ERRNO as i32) {
            return Err(EINVAL);
        }
        let result = if raw.status == 0 {
            // SAFETY: The signed native timestamp was checked to be nonnegative.
            Ok(unsafe { Instant::<Monotonic>::from_ktime(raw.completed_at) })
        } else {
            if raw.completed_at != 0 {
                return Err(EINVAL);
            }
            Err(Error::from_errno(raw.status))
        };
        Self::new(raw.use_id, result)
    }
}

#[cfg(CONFIG_KUNIT)]
#[kunit_tests(rust_drm_capture_completion)]
mod tests {
    use super::*;

    #[test]
    fn completion_names_are_nonzero_even_for_failed_attempts() -> Result {
        let now = Instant::<Monotonic>::now();
        assert!(matches!(Completion::new(0, Ok(now)), Err(EINVAL)));
        assert!(matches!(Completion::new(0, Err(EIO)), Err(EINVAL)));
        assert_eq!(Completion::new(u64::MAX, Err(EIO))?.use_id(), u64::MAX);
        Ok(())
    }

    #[test]
    fn image_time_and_failure_are_distinct_outcomes() -> Result {
        let now = Instant::<Monotonic>::now();
        let image = Completion::new(19, Ok(now))?;
        assert_eq!(image.use_id(), 19);
        assert_eq!(image.result()?.as_nanos(), now.as_nanos());
        let failure = Completion::new(20, Err(EAGAIN))?;
        assert_eq!(failure.use_id(), 20);
        assert!(matches!(failure.result(), Err(EAGAIN)));
        Ok(())
    }
}
