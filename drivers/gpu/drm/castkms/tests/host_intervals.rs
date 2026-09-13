// SPDX-License-Identifier: GPL-2.0-only

//! Completed private images retain the display interval of their source pixels.

use super::*;
use crate::host_compositor::{
    compose,
    layout::Layout,
    pool::Pool, //
};

fn accepted(fixture: &Fixture) -> Result<scene::Configuration> {
    fixture
        .drm
        .device()
        .output
        .with_accepted(|accepted| accepted.and_then(|accepted| accepted.configuration.clone()))
        .ok_or(EINVAL)
}

#[kunit_tests(rust_castkms_host_intervals)]
mod cases {
    use super::*;

    #[test]
    fn an_old_image_does_not_adopt_a_reenabled_outputs_configuration() -> Result {
        let fixture = Fixture::new()?;
        let owner = fixture.drm.synthetic_master_snapshot(true)?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(Some(owner)))?;
        fixture.select(&fb, false, 0)?;
        let first = accepted(&fixture)?;
        let output = &fixture.drm.device().output;
        let pool = Pool::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(640, 480)?,
        )?;
        let old = compose::current(output, &pool)?.ok_or(EINVAL)?;
        check(old.configuration() == Some(&first))?;
        fixture.drm.update(|mut transaction| {
            transaction
                .as_mut()
                .set_crtc_config(fixture.drm.crtc()?, None)
        })?;
        fixture.select(&fb, false, 0)?;
        let second = accepted(&fixture)?;
        let new = compose::current(output, &pool)?.ok_or(EINVAL)?;
        check(first != second)?;
        check(old.output_identity() == new.output_identity())?;
        check(old.layout() == new.layout())?;
        check(old.owner() == new.owner())?;
        check(old.configuration() == Some(&first))?;
        check(new.configuration() == Some(&second))?;
        fixture.state.close();
        check(old.configuration() == Some(&first))?;
        check(new.configuration() == Some(&second))?;
        Ok(())
    }

    #[test]
    fn changing_content_keeps_completed_images_in_the_same_interval() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let configuration = accepted(&fixture)?;
        let output = &fixture.drm.device().output;
        let pool = Pool::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(640, 480)?,
        )?;
        let first = compose::current(output, &pool)?.ok_or(EINVAL)?;
        fixture.select(&fb, false, 0)?;
        let second = compose::current(output, &pool)?.ok_or(EINVAL)?;
        check(first.content_serial() != second.content_serial())?;
        check(first.configuration() == Some(&configuration))?;
        check(second.configuration() == Some(&configuration))?;
        Ok(())
    }
}
