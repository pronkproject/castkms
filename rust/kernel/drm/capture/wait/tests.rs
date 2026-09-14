// SPDX-License-Identifier: GPL-2.0 OR MIT

use crate::{
    drm::capture::{
        Status,
        Stream, //
    },
    prelude::*,
    sync::CondVar, //
};

#[kunit_tests(rust_drm_capture_wait)]
mod cases {
    use super::*;

    #[test]
    fn provider_observation_returns_owned_data_without_claiming() -> Result {
        let changed = KBox::pin_init(kernel::sync::new_condvar!(), GFP_KERNEL)?;
        let stream = Stream::new(1, 16)?;
        let request = stream.queue()?;
        let value =
            request.wait_for_provider(&changed, || Ok(Some(KBox::new(42u32, GFP_KERNEL)?)))?;
        assert_eq!(*value, 42);
        assert_eq!(request.status()?, Status::Pending);
        Ok(())
    }

    #[test]
    fn failed_requests_do_not_enter_the_provider_callback() -> Result {
        let changed: Pin<KBox<CondVar>> = KBox::pin_init(kernel::sync::new_condvar!(), GFP_KERNEL)?;
        let stream = Stream::new(1, 16)?;
        let request = stream.queue()?;
        request.cancel()?;
        let mut observed = false;
        let result = request.wait_for_provider(&changed, || {
            observed = true;
            Ok(Some(()))
        });
        assert_eq!(result, Err(ECANCELED));
        assert!(!observed);
        assert_eq!(request.status()?, Status::Complete(Err(ECANCELED)));
        Ok(())
    }

    #[test]
    fn provider_error_leaves_the_request_queued() -> Result {
        let changed = KBox::pin_init(kernel::sync::new_condvar!(), GFP_KERNEL)?;
        let stream = Stream::new(1, 16)?;
        let request = stream.queue()?;
        let result = request.wait_for_provider(&changed, || Err::<Option<()>, _>(ENODEV));
        assert_eq!(result, Err(ENODEV));
        assert_eq!(request.status()?, Status::Pending);
        Ok(())
    }

    #[test]
    fn successful_wait_retains_image_and_queue_credit() -> Result {
        let stream = Stream::new(1, 16)?;
        let request = stream.queue()?;
        let mut job = stream.claim()?;
        job.data_mut().fill(0x45);
        job.complete(Ok(()));
        request.wait()??;
        request.wait()??;
        assert!(matches!(stream.queue(), Err(EAGAIN)));
        let mut output = [0; 16];
        request.copy_result(&mut output)?;
        assert_eq!(output, [0x45; 16]);
        Ok(())
    }

    #[test]
    fn provider_failure_and_missing_request_have_different_results() -> Result {
        let stream = Stream::new(1, 16)?;
        let request = stream.queue()?;
        stream.claim()?.complete(Err(EIO));
        assert_eq!(request.wait(), Ok(Err(EIO)));
        stream.shutdown();
        assert_eq!(request.wait(), Err(ENOENT));
        Ok(())
    }

    #[test]
    fn canceled_request_wait_does_not_acknowledge_result() -> Result {
        let stream = Stream::new(1, 16)?;
        let request = stream.queue()?;
        request.cancel()?;
        assert_eq!(request.wait(), Ok(Err(ECANCELED)));
        assert_eq!(request.status()?, Status::Complete(Err(ECANCELED)));
        assert!(matches!(stream.queue(), Err(EAGAIN)));
        Ok(())
    }
}
