// SPDX-License-Identifier: GPL-2.0-only

use super::*;
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
    fn worker_returns_private_images_of_the_current_scene() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let pool = Pool::new(fixture.drm.device(), 640, 480)?;
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
        let pool = Pool::new(fixture.drm.device(), 640, 480)?;
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
        let pool = Pool::new(fixture.drm.device(), 640, 480)?;
        let owner = Owner::new(fixture.drm.device().output.clone(), pool)?;
        let handle = owner.handle();
        handle.request()?;
        owner.flush();
        check(matches!(handle.take_outcome(), Some(Outcome::Blank)))?;
        check(handle.take_outcome().is_none())?;
        Ok(())
    }
}
