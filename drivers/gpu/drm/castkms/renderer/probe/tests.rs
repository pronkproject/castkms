// SPDX-License-Identifier: GPL-2.0-only

//! Candidate probe publication retains native completion and reports its result.

use super::*;
use kernel::{
    dma_fence::testing::ManualFence,
    sync::Arc, //
};

fn probe() -> Result<Arc<Probe>> {
    Arc::pin_init(Probe::new(), GFP_KERNEL)
}

#[kunit_tests(rust_castkms_renderer_probe)]
mod cases {
    use super::*;

    #[test]
    fn unsubmitted_probe_has_no_result() -> Result {
        let probe = probe()?;
        assert_eq!(probe.result(), Err(ENODATA));
        assert_eq!(probe.completed_source(), Err(ENODATA));
        Ok(())
    }

    #[test]
    fn synchronous_private_probe_completes_without_content_identity() -> Result {
        let probe = probe()?;
        probe.submit_then(Source::Private, None, || Ok(()))?;
        assert_eq!(probe.result(), Ok(true));
        assert_eq!(probe.completed_source(), Ok(Source::Private));
        Ok(())
    }

    #[test]
    fn native_completion_remains_pending_until_the_fence_signals() -> Result {
        let probe = probe()?;
        let mut completion = ManualFence::new()?;
        probe.submit_then(Source::Snapshot(None), Some(completion.fence()), || Ok(()))?;
        assert_eq!(probe.result(), Ok(false));
        assert_eq!(probe.completed_source(), Err(EAGAIN));
        completion.complete(Ok(()))?;
        assert_eq!(probe.completed_source(), Ok(Source::Snapshot(None)));
        Ok(())
    }

    #[test]
    fn native_failure_does_not_become_a_successful_probe() -> Result {
        let probe = probe()?;
        let mut completion = ManualFence::new()?;
        probe.submit_then(Source::Private, Some(completion.fence()), || Ok(()))?;
        completion.complete(Err(EIO))?;
        assert_eq!(probe.result(), Err(EIO));
        assert_eq!(probe.completed_source(), Err(EIO));
        Ok(())
    }

    #[test]
    fn validation_failure_returns_the_submission_slot() -> Result {
        let probe = probe()?;
        assert_eq!(
            probe.submit_then(Source::Private, None, || Err(ESTALE)),
            Err(ESTALE)
        );
        probe.submit_then(Source::Snapshot(None), None, || Ok(()))?;
        assert_eq!(probe.completed_source(), Ok(Source::Snapshot(None)));
        Ok(())
    }

    #[test]
    fn one_candidate_cannot_publish_two_probe_submissions() -> Result {
        let probe = probe()?;
        probe.submit_then(Source::Private, None, || Ok(()))?;
        assert_eq!(
            probe.submit_then(Source::Private, None, || Ok(())),
            Err(EALREADY)
        );
        Ok(())
    }
}
