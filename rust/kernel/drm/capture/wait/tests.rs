// SPDX-License-Identifier: GPL-2.0 OR MIT

use crate::{
    drm::capture::{
        Status,
        Stream, //
    },
    prelude::*, //
};

#[kunit_tests(rust_drm_capture_wait)]
mod cases {
    use super::*;

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
