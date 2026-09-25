// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Cancellable notifications for source admission transitions.

use super::Source;
use crate::{error::from_err_ptr, ffi::c_void, prelude::*, sync::Arc};
use core::ptr::NonNull;

/// Called without sleeping when source admission reopens, seals, or fails.
pub trait AdmissionListener: Send + Sync {
    /// Schedule nonblocking observation of the source's current admission state.
    fn admission_changed(listener: &Arc<Self>);
}

/// A source subscription whose listener stays alive through callback detachment.
pub struct AdmissionObserver<T: AdmissionListener> {
    raw: NonNull<bindings::drm_prepare_source_observer>,
    _listener: Arc<T>,
}

// SAFETY: Native removal synchronizes with callbacks and Arc retains the listener.
unsafe impl<T: AdmissionListener> Send for AdmissionObserver<T> {}

impl<T: AdmissionListener> AdmissionObserver<T> {
    /// Subscribe before inspecting whether admission is still closed.
    pub fn new(source: &Source, listener: Arc<T>) -> Result<Self> {
        unsafe extern "C" fn notify<T: AdmissionListener>(data: *mut c_void) {
            // SAFETY: Native callback detachment finishes before the owned listener is dropped.
            let listener = unsafe { crate::sync::ArcBorrow::from_raw(data.cast::<T>()) };
            T::admission_changed(&Arc::from(listener));
        }

        // SAFETY: The listener allocation is pinned by Arc. Destruction removes the native
        // callback before releasing that Arc, and notification never retains its pointer.
        let raw = from_err_ptr(unsafe {
            bindings::drm_prepare_source_observe(
                source.0.get(),
                Some(notify::<T>),
                Arc::as_ptr(&listener).cast_mut().cast(),
            )
        })?;
        Ok(Self {
            // SAFETY: Native creation returned an owned non-null observer.
            raw: unsafe { NonNull::new_unchecked(raw) },
            _listener: listener,
        })
    }
}

impl<T: AdmissionListener> Drop for AdmissionObserver<T> {
    fn drop(&mut self) {
        // SAFETY: The source and callback are retained until synchronous removal completes.
        unsafe { bindings::drm_prepare_source_observer_destroy(self.raw.as_ptr()) };
    }
}

#[cfg(CONFIG_KUNIT)]
#[kunit_tests(rust_drm_prepare_admission_observer)]
mod tests {
    use super::*;
    use crate::drm::preparation::AdmissionStatus;
    use core::sync::atomic::{AtomicU32, Ordering};

    struct Listener(AtomicU32);

    impl AdmissionListener for Listener {
        fn admission_changed(listener: &Arc<Self>) {
            listener.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn hold_release_and_seal_notify_without_retaining_the_observer() -> Result {
        let source = Source::new(1)?;
        let listener = Arc::new(Listener(AtomicU32::new(0)), GFP_KERNEL)?;
        let observer = AdmissionObserver::new(&source, listener.clone())?;
        let hold = source.hold_admission()?;
        assert!(matches!(source.admission_status()?, AdmissionStatus::Held));
        drop(hold);
        assert!(matches!(source.admission_status()?, AdmissionStatus::Open));
        assert_eq!(listener.0.load(Ordering::Relaxed), 1);
        source.seal();
        assert!(matches!(
            source.admission_status()?,
            AdmissionStatus::Sealed
        ));
        assert_eq!(listener.0.load(Ordering::Relaxed), 2);
        drop(observer);
        let other = Source::new(1)?;
        let detached = AdmissionObserver::new(&other, listener.clone())?;
        drop(detached);
        other.seal();
        assert_eq!(listener.0.load(Ordering::Relaxed), 2);
        Ok(())
    }

    #[test]
    fn abandoned_read_notifies_waiters_of_terminal_failure() -> Result {
        let source = Source::new(1)?;
        let listener = Arc::new(Listener(AtomicU32::new(0)), GFP_KERNEL)?;
        let _observer = AdmissionObserver::new(&source, listener.clone())?;
        drop(source.claim()?);
        assert_eq!(listener.0.load(Ordering::Relaxed), 1);
        assert!(matches!(source.admission_status(), Err(EIO)));
        Ok(())
    }
}
