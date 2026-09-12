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
        owner.request()?;
        owner.flush();
        let Some(Outcome::Image(first)) = owner.take_outcome() else {
            return Err(EINVAL);
        };
        fixture.select(&fb, false, 0)?;
        owner.request()?;
        owner.flush();
        let Some(Outcome::Image(second)) = owner.take_outcome() else {
            return Err(EINVAL);
        };
        assert_ne!(first.content_serial(), second.content_serial());
        owner.request()?;
        owner.flush();
        assert!(matches!(owner.take_outcome(), Some(Outcome::Failed(EBUSY))));
        drop(first);
        owner.request()?;
        owner.flush();
        assert!(matches!(owner.take_outcome(), Some(Outcome::Image(_))));
        Ok(())
    }

    #[test]
    fn queued_shutdown_discards_results_and_closes_storage() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let pool = Pool::new(fixture.drm.device(), 640, 480)?;
        let owner = Owner::new(fixture.drm.device().output.clone(), pool.clone())?;
        for _ in 0..16 {
            owner.request()?;
        }
        owner.close();
        assert!(owner.take_outcome().is_none());
        assert_eq!(owner.request(), Err(ENODEV));
        assert!(matches!(pool.reserve(), Err(ENODEV)));
        Ok(())
    }

    #[test]
    fn blank_output_finishes_without_a_source_image() -> Result {
        let fixture = Fixture::new()?;
        let pool = Pool::new(fixture.drm.device(), 640, 480)?;
        let owner = Owner::new(fixture.drm.device().output.clone(), pool)?;
        owner.request()?;
        owner.flush();
        assert!(matches!(owner.take_outcome(), Some(Outcome::Blank)));
        assert!(owner.take_outcome().is_none());
        Ok(())
    }
}
