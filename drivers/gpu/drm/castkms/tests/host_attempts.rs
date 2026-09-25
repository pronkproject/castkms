// SPDX-License-Identifier: GPL-2.0-only

//! Independent observations of coalesced host work, without capture authorization.

use super::*;
use crate::host_compositor::{
    layout::Layout,
    worker::Outcome, //
};
use kernel::io::{
    io_project,
    Io, //
};

#[kunit_tests(rust_castkms_host_attempts)]
mod cases {
    use super::*;

    #[test]
    fn coalesced_requests_do_not_consume_each_others_completion() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let host = fixture
            .drm
            .device()
            .host
            .configure(fixture.drm.device(), Layout::new(640, 480)?)?;
        for _ in 0..32 {
            let first = host.request_outcome()?;
            let second = host.request_outcome()?;
            check(matches!(second.wait()?, Outcome::Image(_)))?;
            check(matches!(first.wait()?, Outcome::Image(_)))?;
            check(matches!(second.try_outcome()?, Some(Outcome::Image(_))))?;
            drop(host.take_outcome());
            check(matches!(first.try_outcome()?, Some(Outcome::Image(_))))?;
        }
        Ok(())
    }

    #[test]
    fn a_new_request_does_not_observe_the_previous_cached_attempt() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let host = fixture
            .drm
            .device()
            .host
            .configure(fixture.drm.device(), Layout::new(640, 480)?)?;
        let first = host.request_outcome()?;
        let Outcome::Image(old) = first.wait()? else {
            return Err(EINVAL);
        };
        let old_serial = old.content_serial();
        drop(old);
        {
            let mapping = fb.vmap::<gem::Object>()?;
            io_project!(mapping.view(), [try: 0..2560]).copy_from_slice(&[0x35; 2560]);
        }
        fixture.select(&fb, false, 0)?;
        let next = host.request_outcome()?;
        if let Some(outcome) = next.try_outcome()? {
            let Outcome::Image(image) = outcome else {
                return Err(EINVAL);
            };
            check(image.content_serial() != old_serial)?;
        }
        let Outcome::Image(image) = next.wait()? else {
            return Err(EINVAL);
        };
        check(image.content_serial() != old_serial)?;
        let mut row = [0; 2560];
        image.read_row(0, &mut row)?;
        check(row == [0x35; 2560])?;
        Ok(())
    }

    #[test]
    fn worker_close_ends_every_observer_without_consuming_a_result() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let configuration = &fixture.drm.device().host;
        let host = configuration.configure(fixture.drm.device(), Layout::new(640, 480)?)?;
        let first = host.request_outcome()?;
        let second = host.request_outcome()?;
        configuration.stop_worker()?;
        check(matches!(first.wait(), Err(ENODEV)))?;
        check(matches!(second.try_outcome(), Err(ENODEV)))?;
        check(matches!(host.request_outcome(), Err(ENODEV)))?;
        Ok(())
    }

    #[test]
    fn closing_while_admission_is_held_detaches_the_pending_observer() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let source: kernel::sync::aref::ARef<kernel::drm::preparation::Source> = fixture
            .drm
            .device()
            .output
            .inspect_accepted(|accepted| accepted.map(|(source, _)| source.into()))
            .ok_or(EINVAL)?;
        let hold = source.hold_admission()?;
        let configuration = &fixture.drm.device().host;
        let host = configuration.configure(fixture.drm.device(), Layout::new(640, 480)?)?;
        let pending = host.request_outcome()?;
        host.flush_for_test();
        check(pending.try_outcome()?.is_none())?;
        configuration.stop_worker()?;
        check(matches!(pending.wait(), Err(ENODEV)))?;
        drop(hold);
        Ok(())
    }

    #[test]
    fn a_failed_attempt_is_observable_by_every_covered_request() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let host = fixture
            .drm
            .device()
            .host
            .configure(fixture.drm.device(), Layout::new(1, 1)?)?;
        let first = host.request_outcome()?;
        let second = host.request_outcome()?;
        check(matches!(first.wait()?, Outcome::Failed(EINVAL)))?;
        check(matches!(second.wait()?, Outcome::Failed(EINVAL)))?;
        Ok(())
    }
}
