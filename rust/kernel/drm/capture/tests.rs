// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;

#[kunit_tests(rust_drm_capture)]
mod cases {
    use super::*;

    #[test]
    fn completed_image_outlives_provider() -> Result {
        let stream = Stream::new(1, 16)?;
        let request = stream.queue()?;
        assert_eq!(request.status()?, Status::Pending);
        let mut job = stream.claim()?;
        assert_eq!(job.data_mut(), &[0; 16]);
        job.data_mut().fill(0x5a);
        job.complete(Ok(()));
        assert_eq!(request.status()?, Status::Complete(Ok(())));
        let mut output = [0; 16];
        assert_eq!(request.copy_result(&mut output)?, 16);
        assert_eq!(output, [0x5a; 16]);
        assert_eq!(stream.queue().err(), Some(EAGAIN));
        drop(request);
        let _replacement = stream.queue()?;
        Ok(())
    }

    #[test]
    fn dropped_request_preserves_active_storage() -> Result {
        let stream = Stream::new(1, 16)?;
        let request = stream.queue()?;
        let mut job = stream.claim()?;
        drop(request);
        assert_eq!(stream.queue().err(), Some(EAGAIN));
        job.data_mut().fill(0x22);
        job.complete(Ok(()));
        let _replacement = stream.queue()?;
        Ok(())
    }

    #[test]
    fn dropped_provider_records_cancellation() -> Result {
        let stream = Stream::new(1, 16)?;
        let request = stream.queue()?;
        drop(stream.claim()?);
        assert_eq!(request.status()?, Status::Complete(Err(ECANCELED)));
        let mut output = [0x77; 16];
        assert_eq!(request.copy_result(&mut output), Err(ECANCELED));
        assert_eq!(output, [0x77; 16]);
        Ok(())
    }

    #[test]
    fn request_retains_originating_stream() -> Result {
        let first = Stream::new(1, 16)?;
        let second = Stream::new(1, 16)?;
        let request = first.queue()?;
        let other = second.queue()?;
        let mut job = first.claim()?;
        drop(first);
        job.data_mut().fill(0x33);
        job.complete(Ok(()));
        let mut output = [0; 16];
        request.copy_result(&mut output)?;
        assert_eq!(output, [0x33; 16]);
        assert_eq!(other.status()?, Status::Pending);
        drop(other);
        let _replacement = second.queue()?;
        Ok(())
    }

    #[test]
    fn shutdown_preserves_active_job() -> Result {
        let stream = Stream::new(1, 16)?;
        let request = stream.queue()?;
        let mut job = stream.claim()?;
        stream.shutdown();
        drop(request);
        drop(stream);
        job.data_mut().fill(0xff);
        job.complete(Ok(()));
        Ok(())
    }

    #[test]
    fn revocation_overrides_provider_success() -> Result {
        let stream = Stream::new(1, 16)?;
        let request = stream.queue()?;
        let mut job = stream.claim()?;
        stream.revoke();
        job.data_mut().fill(0xff);
        job.complete(Ok(()));
        assert_eq!(request.status()?, Status::Complete(Err(EKEYREVOKED)));
        assert_eq!(stream.queue().err(), Some(EKEYREVOKED));
        Ok(())
    }

    #[test]
    fn cancel_retains_status_until_request_drop() -> Result {
        let stream = Stream::new(1, 16)?;
        let request = stream.queue()?;
        request.cancel()?;
        assert_eq!(request.status()?, Status::Complete(Err(ECANCELED)));
        assert_eq!(stream.queue().err(), Some(EAGAIN));
        drop(request);
        let _replacement = stream.queue()?;
        Ok(())
    }
}
