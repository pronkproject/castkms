// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::host_compositor::layout::Layout;
use crate::host_compositor::{
    pool::Pool,
    worker::{
        Outcome,
        Owner, //
    }, //
};

#[kunit_tests(rust_castkms_host_worker)]
mod cases {
    use super::*;

    #[test]
    fn admission_cutoff_detaches_results_without_releasing_their_storage() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let pool = Pool::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(640, 480)?,
        )?;
        let owner = Owner::new(fixture.drm.device().output.clone(), pool)?;
        let handle = owner.handle();
        handle.request()?;
        owner.flush();
        check(handle.last_image().is_some())?;
        let retired = owner.stop_admission();
        check(handle.request() == Err(ENODEV))?;
        check(handle.last_image().is_none())?;
        check(handle.take_outcome().is_none())?;
        owner.close();
        check(matches!(
            fixture.host_budget.reserve(16 * 1024 * 1024),
            Err(EBUSY)
        ))?;
        drop(retired);
        let _whole_budget = fixture.host_budget.reserve(16 * 1024 * 1024)?;
        Ok(())
    }

    #[test]
    fn cutoff_ends_pending_observation_before_the_owner_is_drained() -> Result {
        let fixture = Fixture::new()?;
        let pool = Pool::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(3, 2)?,
        )?;
        let owner = Owner::new(fixture.drm.device().output.clone(), pool)?;
        let request = owner.handle().request_outcome()?;
        let retired = owner.stop_admission();
        check(matches!(request.try_outcome(), Err(ENODEV)))?;
        check(matches!(request.wait(), Err(ENODEV)))?;
        check(owner.handle().request() == Err(ENODEV))?;
        drop(owner.stop_admission());
        drop(retired);
        owner.flush();
        check(owner.handle().take_outcome().is_none())?;
        Ok(())
    }

    #[test]
    fn the_last_shared_image_keeps_its_storage_charge_after_shutdown() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let pool = Pool::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(640, 480)?,
        )?;
        let owner = Owner::new(fixture.drm.device().output.clone(), pool)?;
        let handle = owner.handle();
        handle.request()?;
        owner.flush();
        let Some(Outcome::Image(image)) = handle.take_outcome() else {
            return Err(EINVAL);
        };
        let retained = image.clone();
        drop(image);
        drop(owner);
        check(handle.request() == Err(ENODEV))?;
        check(matches!(
            fixture.host_budget.reserve(16 * 1024 * 1024),
            Err(EBUSY)
        ))?;
        let mut row = [0xff; 2560];
        retained.read_row(0, &mut row)?;
        check(row == [0; 2560])?;
        drop(retained);
        let _whole_budget = fixture.host_budget.reserve(16 * 1024 * 1024)?;
        Ok(())
    }

    #[test]
    fn surviving_handles_do_not_postpone_shutdown() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let pool = Pool::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(640, 480)?,
        )?;
        let owner = Owner::new(fixture.drm.device().output.clone(), pool.clone())?;
        let handle = owner.handle();
        let peer = handle.clone();
        handle.request()?;
        owner.flush();
        let Some(Outcome::Image(image)) = handle.take_outcome() else {
            return Err(EINVAL);
        };
        peer.request()?;
        drop(owner);
        check(handle.request() == Err(ENODEV))?;
        check(peer.request() == Err(ENODEV))?;
        check(handle.take_outcome().is_none())?;
        check(matches!(pool.reserve(), Err(ENODEV)))?;
        let mut row = [0xff; 2560];
        image.read_row(0, &mut row)?;
        check(row == [0; 2560])?;
        drop(handle);
        check(peer.request() == Err(ENODEV))?;
        Ok(())
    }

    #[test]
    fn dropping_a_handle_preserves_the_owner() -> Result {
        let fixture = Fixture::new()?;
        let pool = Pool::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(640, 480)?,
        )?;
        let owner = Owner::new(fixture.drm.device().output.clone(), pool)?;
        let handle = owner.handle();
        handle.request()?;
        drop(handle);
        owner.flush();
        let replacement = owner.handle();
        check(matches!(replacement.take_outcome(), Some(Outcome::NoScene)))?;
        replacement.request()?;
        owner.flush();
        check(matches!(replacement.take_outcome(), Some(Outcome::NoScene)))?;
        Ok(())
    }

    #[test]
    fn cloned_handles_consume_one_shared_result() -> Result {
        let fixture = Fixture::new()?;
        let pool = Pool::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(640, 480)?,
        )?;
        let owner = Owner::new(fixture.drm.device().output.clone(), pool)?;
        let first = owner.handle();
        let second = first.clone();
        first.request()?;
        owner.flush();
        check(matches!(second.take_outcome(), Some(Outcome::NoScene)))?;
        check(first.take_outcome().is_none())?;
        owner.close();
        let closed = owner.handle();
        check(closed.request() == Err(ENODEV))?;
        Ok(())
    }

    #[test]
    fn worker_returns_private_images_of_the_current_scene() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let pool = Pool::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(640, 480)?,
        )?;
        let owner = Owner::new(fixture.drm.device().output.clone(), pool)?;
        let handle = owner.handle();
        handle.request()?;
        owner.flush();
        let Some(Outcome::Image(first)) = handle.take_outcome() else {
            return Err(EINVAL);
        };
        fixture.select(&fb, false, 0)?;
        handle.request()?;
        owner.flush();
        let Some(Outcome::Image(second)) = handle.take_outcome() else {
            return Err(EINVAL);
        };
        check(first.content_serial() != second.content_serial())?;
        handle.request()?;
        owner.flush();
        check(matches!(
            handle.take_outcome(),
            Some(Outcome::Failed(EBUSY))
        ))?;
        drop(first);
        handle.request()?;
        owner.flush();
        check(matches!(handle.take_outcome(), Some(Outcome::Image(_))))?;
        Ok(())
    }

    #[test]
    fn queued_shutdown_discards_results_and_closes_storage() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let pool = Pool::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(640, 480)?,
        )?;
        let owner = Owner::new(fixture.drm.device().output.clone(), pool.clone())?;
        let handle = owner.handle();
        for _ in 0..16 {
            handle.request()?;
        }
        owner.close();
        check(handle.take_outcome().is_none())?;
        check(handle.request() == Err(ENODEV))?;
        check(matches!(pool.reserve(), Err(ENODEV)))?;
        Ok(())
    }

    #[test]
    fn blank_output_finishes_without_a_source_image() -> Result {
        let fixture = Fixture::new()?;
        let pool = Pool::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(640, 480)?,
        )?;
        let owner = Owner::new(fixture.drm.device().output.clone(), pool)?;
        let handle = owner.handle();
        handle.request()?;
        owner.flush();
        check(matches!(handle.take_outcome(), Some(Outcome::NoScene)))?;
        check(handle.take_outcome().is_none())?;
        Ok(())
    }
}
