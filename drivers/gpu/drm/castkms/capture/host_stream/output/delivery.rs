// SPDX-License-Identifier: GPL-2.0-only

//! Detached destination access with queue-local observation and cancellation.

use crate::capture::{
    destination::Image,
    provider::Frame, //
};
use core::sync::atomic::{
    AtomicBool,
    Ordering, //
};
use kernel::{
    dma_fence::Fence,
    drm::capture::delivery::{
        self,
        Delivery, //
    },
    prelude::*,
    sync::{
        aref::ARef,
        Arc,
        Mutex, //
    }, //
};

#[pin_data]
struct Shared<T> {
    cancelled: AtomicBool,
    #[pin]
    result: Mutex<Option<(T, Result<bool>)>>,
}

impl<T: Unpin> Shared<T> {
    fn new() -> Result<Arc<Self>> {
        Arc::pin_init(
            pin_init!(Self {
                cancelled: AtomicBool::new(false),
                result <- kernel::new_mutex!(None),
            }),
            GFP_KERNEL,
        )
    }

    fn complete(&self, value: T, result: Result<bool>) {
        let mut output = self.result.lock();
        let result = if self.cancelled.load(Ordering::Acquire) && result.is_ok() {
            Err(ECANCELED)
        } else {
            result
        };
        *output = Some((value, result));
    }

    fn cancel(&self) -> Result {
        let mut output = self.result.lock();
        if let Some((_, result)) = &mut *output {
            if *result != Ok(false) {
                return Err(EALREADY);
            }
            // A new dependency deferred access rather than producing a terminal result.
            *result = Err(ECANCELED);
        }
        if self.cancelled.swap(true, Ordering::AcqRel) {
            Err(EALREADY)
        } else {
            Ok(())
        }
    }
}

struct Copy {
    frame: Frame,
    destination: Arc<Image>,
    reuse: Option<ARef<Fence>>,
    shared: Arc<Shared<Frame>>,
}

// SAFETY: The callback, destructor and dependencies belong to CastKMS. Native dispatch
// holds its module reference until the entire callback, including destruction, returns.
#[vtable]
unsafe impl Delivery for Copy {
    fn run(self) {
        let result = self.frame.request().try_copy_to_destination_unless(
            &self.destination,
            self.reuse.as_deref(),
            || self.shared.cancelled.load(Ordering::Acquire),
        );
        // Exporter access has ended before taking the observation lock. Moving the result
        // into its empty slot performs no image or destination access under that lock.
        self.shared.complete(self.frame, result);
    }
}

/// Dropping observation requests cancellation without waiting for exporter access.
///
/// The native task retains its exact frame, destination and callback module independently.
/// An abandoned destination must not be reused until another completion mechanism proves
/// access ended; dropping this handle is deliberately not such an acknowledgment.
pub(super) struct Handle {
    shared: Arc<Shared<Frame>>,
}

impl Handle {
    pub(super) fn new(
        frame: Frame,
        destination: Arc<Image>,
        reuse: Option<ARef<Fence>>,
    ) -> Result<Self> {
        let shared = Shared::new()?;
        delivery::submit(Copy {
            frame,
            destination,
            reuse,
            shared: shared.clone(),
        })?;
        Ok(Self { shared })
    }

    pub(super) fn is_running(&self) -> bool {
        self.shared.result.lock().is_none()
    }

    pub(super) fn take_result(&self) -> Option<(Frame, Result<bool>)> {
        self.shared.result.lock().take()
    }

    pub(super) fn cancel(&self) -> Result {
        self.shared.cancel()
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        let _ = self.cancel();
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
#[kunit_tests(rust_castkms_capture_delivery_cancel)]
mod tests {
    use super::*;

    #[test]
    fn deferred_access_remains_cancellable_after_the_task_returns() -> Result {
        let shared = Shared::new()?;
        shared.complete(17u32, Ok(false));
        shared.cancel()?;
        assert_eq!(shared.cancel(), Err(EALREADY));
        assert_eq!(shared.result.lock().take(), Some((17, Err(ECANCELED))));
        Ok(())
    }

    #[test]
    fn cancellation_before_publication_is_retained_until_access_ends() -> Result {
        let shared = Shared::new()?;
        shared.cancel()?;
        assert!(shared.result.lock().is_none());
        shared.complete(19u32, Ok(true));
        assert_eq!(shared.result.lock().take(), Some((19, Err(ECANCELED))));
        Ok(())
    }

    #[test]
    fn terminal_access_keeps_its_first_result() -> Result {
        for result in [Ok(true), Err(EAGAIN)] {
            let shared = Shared::new()?;
            shared.complete(23u32, result);
            assert_eq!(shared.cancel(), Err(EALREADY));
            assert_eq!(shared.result.lock().take(), Some((23, result)));
        }
        Ok(())
    }
}
