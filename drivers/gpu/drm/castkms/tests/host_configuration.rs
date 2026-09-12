// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::host_compositor::{
    configuration::Owner,
    layout::Layout,
    worker::Outcome, //
};

#[kunit_tests(rust_castkms_host_configuration)]
mod cases {
    use super::*;

    #[test]
    fn an_unconfigured_owner_closes_without_a_graphics_device() -> Result {
        let output = kernel::sync::Arc::pin_init(output::Output::new(), GFP_KERNEL)?;
        let owner = Owner::new(output)?;
        let configuration = owner.configuration();
        check(matches!(configuration.current(), Err(EAGAIN)))?;
        drop(owner);
        check(matches!(configuration.current(), Err(ENODEV)))?;
        Ok(())
    }

    #[test]
    fn identical_layouts_preserve_the_worker_and_its_result() -> Result {
        let fixture = Fixture::new()?;
        let owner = Owner::new(fixture.drm.device().output.clone())?;
        let configuration = owner.configuration();
        let layout = Layout::new(3, 2)?;
        let first = configuration.configure(fixture.drm.device(), layout)?;
        first.request()?;
        first.flush_for_test();
        let second = configuration.configure(fixture.drm.device(), layout)?;
        check(matches!(second.take_outcome(), Some(Outcome::Blank)))?;
        check(first.take_outcome().is_none())?;
        first.request()?;
        first.flush_for_test();
        check(matches!(second.take_outcome(), Some(Outcome::Blank)))?;
        Ok(())
    }

    #[test]
    fn changed_geometry_closes_old_handles_even_at_equal_allocation_size() -> Result {
        let fixture = Fixture::new()?;
        let owner = Owner::new(fixture.drm.device().output.clone())?;
        let configuration = owner.configuration();
        let first = configuration.configure(fixture.drm.device(), Layout::new(3, 2)?)?;
        first.request()?;
        first.flush_for_test();
        let second = configuration.configure(fixture.drm.device(), Layout::new(2, 3)?)?;
        check(first.request() == Err(ENODEV))?;
        check(first.take_outcome().is_none())?;
        second.request()?;
        second.flush_for_test();
        check(matches!(
            configuration.current()?.take_outcome(),
            Some(Outcome::Blank)
        ))?;
        Ok(())
    }

    #[test]
    fn retained_images_bound_replacement_and_release_allows_retry() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let owner = Owner::new(fixture.drm.device().output.clone())?;
        let configuration = owner.configuration();
        let first = configuration.configure(fixture.drm.device(), Layout::new(640, 480)?)?;
        first.request()?;
        first.flush_for_test();
        let Some(Outcome::Image(image)) = first.take_outcome() else {
            return Err(EINVAL);
        };
        check(matches!(
            configuration.configure(fixture.drm.device(), Layout::new(1920, 1080)?),
            Err(EBUSY)
        ))?;
        check(first.request() == Err(ENODEV))?;
        check(matches!(configuration.current(), Err(EAGAIN)))?;
        let mut row = [0xff; 2560];
        image.read_row(0, &mut row)?;
        check(row == [0; 2560])?;
        drop(image);
        let _replacement =
            configuration.configure(fixture.drm.device(), Layout::new(1920, 1080)?)?;
        Ok(())
    }

    #[test]
    fn surviving_configuration_references_cannot_restart_after_owner_release() -> Result {
        let fixture = Fixture::new()?;
        let owner = Owner::new(fixture.drm.device().output.clone())?;
        let configuration = owner.configuration();
        let handle = configuration.configure(fixture.drm.device(), Layout::new(640, 480)?)?;
        handle.request()?;
        drop(owner);
        check(handle.request() == Err(ENODEV))?;
        check(handle.take_outcome().is_none())?;
        check(matches!(configuration.current(), Err(ENODEV)))?;
        check(matches!(
            configuration.configure(fixture.drm.device(), Layout::new(3, 2)?),
            Err(ENODEV)
        ))?;
        Ok(())
    }
}
