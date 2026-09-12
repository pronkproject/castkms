// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::host_compositor::{
    layout::Layout,
    worker::Outcome, //
};

#[kunit_tests(rust_castkms_host_idle)]
mod cases {
    use super::*;

    #[test]
    fn stopping_an_idle_configuration_is_repeatable_until_shutdown() -> Result {
        let fixture = Fixture::new()?;
        let configuration = &fixture.drm.device().host;
        configuration.stop_worker()?;
        configuration.stop_worker()?;
        check(matches!(configuration.current(), Err(EAGAIN)))?;
        fixture.state.close();
        check(configuration.stop_worker() == Err(ENODEV))?;
        Ok(())
    }

    #[test]
    fn stopping_a_worker_preserves_the_display_for_a_new_worker() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let configuration = &fixture.drm.device().host;
        let layout = Layout::new(640, 480)?;
        let first = configuration.configure(fixture.drm.device(), layout)?;
        first.request()?;
        configuration.stop_worker()?;
        check(first.request() == Err(ENODEV))?;
        check(first.take_outcome().is_none())?;
        check(matches!(configuration.current(), Err(EAGAIN)))?;
        check(fixture.drm.device().output.inspect(|scene| scene.is_some()))?;
        let next = configuration.configure(fixture.drm.device(), layout)?;
        next.request()?;
        next.flush_for_test();
        check(matches!(next.take_outcome(), Some(Outcome::Image(_))))?;
        check(first.request() == Err(ENODEV))?;
        Ok(())
    }

    #[test]
    fn stopping_does_not_refund_a_retained_images_allocation() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let configuration = &fixture.drm.device().host;
        let first = configuration.configure(fixture.drm.device(), Layout::new(640, 480)?)?;
        first.request()?;
        first.flush_for_test();
        let Some(Outcome::Image(image)) = first.take_outcome() else {
            return Err(EINVAL);
        };
        configuration.stop_worker()?;
        check(matches!(
            configuration.configure(fixture.drm.device(), Layout::new(1920, 1080)?),
            Err(EBUSY)
        ))?;
        let mut pixels = [0xff; 2560];
        image.read_row(0, &mut pixels)?;
        check(pixels == [0; 2560])?;
        drop(image);
        let _replacement =
            configuration.configure(fixture.drm.device(), Layout::new(1920, 1080)?)?;
        Ok(())
    }
}
