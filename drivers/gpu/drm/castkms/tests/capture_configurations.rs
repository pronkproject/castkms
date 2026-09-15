// SPDX-License-Identifier: GPL-2.0-only

//! Selective stream revocation by independently retained display configuration.

use super::*;
use crate::capture::streams::Registry;
use kernel::drm::capture::{
    Status,
    Stream, //
};

#[kunit_tests(rust_castkms_capture_configurations)]
mod cases {
    use super::*;

    #[test]
    fn retiring_one_interval_preserves_other_registered_streams() -> Result {
        let registry = Registry::new()?;
        let old = scene::Configuration::new(1, [640, 480], 60_000, 0)?;
        let current = scene::Configuration::new(1, [640, 480], 60_000, 0)?;
        let first = Stream::new(1, 4)?;
        let second = Stream::new(1, 4)?;
        let old_registration = registry.register(&first, &old)?;
        let _current_registration = registry.register(&second, &current)?;
        let old_request = first.queue()?;
        let current_request = second.queue()?;
        registry.revoke_configuration(&old);
        check(old_request.status()? == Status::Complete(Err(EKEYREVOKED)))?;
        check(current_request.status()? == Status::Pending)?;
        drop(old_registration);
        second.claim()?.complete(Ok(()));
        check(current_request.status()? == Status::Complete(Ok(())))?;
        check(matches!(first.queue(), Err(EKEYREVOKED)))?;
        Ok(())
    }

    #[test]
    fn a_claimed_old_interval_finishes_without_exposing_new_success() -> Result {
        let registry = Registry::new()?;
        let configuration = scene::Configuration::new(1, [640, 480], 60_000, 0)?;
        let stream = Stream::new(1, 4)?;
        let _registration = registry.register(&stream, &configuration)?;
        let request = stream.queue()?;
        let mut job = stream.claim()?;
        registry.revoke_configuration(&configuration);
        check(request.status()? == Status::Pending)?;
        job.data_mut().copy_from_slice(&[0x35; 4]);
        job.complete(Ok(()));
        check(request.status()? == Status::Complete(Err(EKEYREVOKED)))?;
        let mut pixels = [0xa7; 4];
        check(request.copy_result(&mut pixels) == Err(EKEYREVOKED))?;
        check(pixels == [0xa7; 4])?;
        Ok(())
    }

    #[test]
    fn repeated_retirement_does_not_revoke_a_newer_registration() -> Result {
        let registry = Registry::new()?;
        let old = scene::Configuration::new(1, [640, 480], 60_000, 0)?;
        let current = scene::Configuration::new(1, [640, 480], 60_000, 0)?;
        let first = Stream::new(1, 4)?;
        let old_registration = registry.register(&first, &old)?;
        registry.revoke_configuration(&old);
        let second = Stream::new(1, 4)?;
        let _current_registration = registry.register(&second, &current)?;
        registry.revoke_configuration(&old);
        drop(old_registration);
        let request = second.queue()?;
        second.claim()?.complete(Ok(()));
        check(request.status()? == Status::Complete(Ok(())))?;
        registry.close();
        check(matches!(second.queue(), Err(EKEYREVOKED)))?;
        Ok(())
    }
}
