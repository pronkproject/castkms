// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::host_compositor::{
    layout::Layout,
    worker::Outcome, //
};

#[kunit_tests(rust_castkms_host_results)]
mod cases {
    use super::*;

    #[test]
    fn completed_layout_survives_worker_replacement_and_device_shutdown() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let configuration = &fixture.drm.device().host;
        let layout = Layout::new(640, 480)?;
        let handle = configuration.configure(fixture.drm.device(), layout)?;
        handle.request()?;
        handle.flush_for_test();
        let Some(Outcome::Image(image)) = handle.take_outcome() else {
            return Err(EINVAL);
        };
        check(image.layout() == layout)?;
        let _replacement = configuration.configure(fixture.drm.device(), Layout::new(3, 2)?)?;
        check(handle.request() == Err(ENODEV))?;
        check(image.layout().dimensions() == (640, 480))?;
        check(image.layout().pitch() == 2560)?;
        fixture.state.close();
        check(image.layout() == layout)?;
        let mut row = [0xff; 2560];
        image.read_row(479, &mut row)?;
        check(row == [0; 2560])?;
        Ok(())
    }
}
