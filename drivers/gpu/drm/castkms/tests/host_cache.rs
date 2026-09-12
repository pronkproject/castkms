// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::host_compositor::{
    layout::Layout,
    worker::Outcome, //
};
use kernel::{
    drm::preparation::Source,
    io::{
        io_project,
        Io, //
    },
    sync::aref::ARef, //
};

#[kunit_tests(rust_castkms_host_cache)]
mod cases {
    use super::*;

    #[test]
    fn failed_attempts_preserve_the_last_complete_image() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let output = &fixture.drm.device().output;
        let source: ARef<Source> = output
            .inspect_accepted(|accepted| accepted.map(|(source, _)| source.into()))
            .ok_or(EINVAL)?;
        let handle = fixture
            .drm
            .device()
            .host
            .configure(fixture.drm.device(), Layout::new(640, 480)?)?;
        check(handle.last_image().is_none())?;
        handle.request()?;
        handle.flush_for_test();
        let Some(Outcome::Image(first)) = handle.take_outcome() else {
            return Err(EINVAL);
        };
        let hold = source.hold_admission()?;
        handle.request()?;
        handle.flush_for_test();
        check(matches!(
            handle.take_outcome(),
            Some(Outcome::Failed(EBUSY))
        ))?;
        let retained = handle.last_image().ok_or(EINVAL)?;
        check(core::ptr::eq(&*first, &*retained))?;
        check(handle.take_outcome().is_none())?;
        check(hold.prepared()?.is_some())?;
        drop(hold);
        fixture.state.close();
        check(handle.last_image().is_none())?;
        let mut row = [0xff; 2560];
        retained.read_row(0, &mut row)?;
        check(row == [0; 2560])?;
        Ok(())
    }

    #[test]
    fn same_framebuffer_updates_replace_cached_content() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let handle = fixture
            .drm
            .device()
            .host
            .configure(fixture.drm.device(), Layout::new(640, 480)?)?;
        handle.request()?;
        handle.flush_for_test();
        let first = handle.last_image().ok_or(EINVAL)?;
        drop(handle.take_outcome());
        {
            let mapping = fb.vmap::<gem::Object>()?;
            io_project!(mapping.view(), [try: 0..2560]).copy_from_slice(&[0x71; 2560]);
        }
        fixture.select(&fb, false, 0)?;
        handle.request()?;
        handle.flush_for_test();
        let second = handle.last_image().ok_or(EINVAL)?;
        check(first.content_serial() != second.content_serial())?;
        let mut row = [0xff; 2560];
        first.read_row(0, &mut row)?;
        check(row == [0; 2560])?;
        second.read_row(0, &mut row)?;
        check(row == [0x71; 2560])?;
        check(matches!(handle.take_outcome(), Some(Outcome::Image(_))))?;
        Ok(())
    }

    #[test]
    fn a_completed_blank_clears_the_cache_without_invalidating_readers() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let handle = fixture
            .drm
            .device()
            .host
            .configure(fixture.drm.device(), Layout::new(640, 480)?)?;
        handle.request()?;
        handle.flush_for_test();
        let retained = handle.last_image().ok_or(EINVAL)?;
        drop(handle.take_outcome());
        handle.request()?;
        handle.flush_for_test();
        let second = handle.last_image().ok_or(EINVAL)?;
        drop(handle.take_outcome());
        check(!core::ptr::eq(&*retained, &*second))?;
        fixture.drm.update(|mut transaction| {
            transaction
                .as_mut()
                .set_crtc_config(fixture.drm.crtc()?, None)
        })?;
        handle.request()?;
        handle.flush_for_test();
        check(matches!(handle.take_outcome(), Some(Outcome::Blank)))?;
        check(handle.last_image().is_none())?;
        let mut row = [0xff; 2560];
        retained.read_row(0, &mut row)?;
        check(row == [0; 2560])?;
        second.read_row(0, &mut row)?;
        check(row == [0; 2560])?;
        Ok(())
    }
}
