// SPDX-License-Identifier: GPL-2.0-only

//! Active blank images use private storage, without retaining a removed framebuffer.

use super::*;
use crate::host_compositor::{
    compose,
    layout::Layout,
    pool::Pool, //
};
use kernel::{
    dma_fence::testing::ManualFence,
    drm::preparation::Source,
    io::{
        io_project,
        Io, //
    },
    sync::aref::ARef, //
};

fn blank(fixture: &Fixture) -> Result {
    fixture
        .drm
        .update(|transaction| transaction.disable_plane(fixture.drm.plane()?))
}

fn pool(fixture: &Fixture, width: u32, height: u32) -> Result<kernel::sync::Arc<Pool>> {
    Pool::new(
        fixture.drm.device(),
        &fixture.host_budget,
        Layout::new(width, height)?,
    )
}

#[kunit_tests(rust_castkms_host_blank)]
mod cases {
    use super::*;

    #[test]
    fn blanking_clears_a_previously_used_private_slot() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        {
            let map = fb.vmap::<gem::Object>()?;
            io_project!(map.view(), [try: 0..2560]).copy_from_slice(&[0x73; 2560]);
        }
        fixture.select(&fb, false, 0)?;
        let output = &fixture.drm.device().output;
        let pool = pool(&fixture, 640, 480)?;
        let image = compose::current(output, &pool)?.ok_or(EINVAL)?;
        let mut row = [0; 2560];
        image.read_row(0, &mut row)?;
        check(row == [0x73; 2560])?;
        let visible_serial = image.content_serial();
        drop(image);
        blank(&fixture)?;
        let image = compose::current(output, &pool)?.ok_or(EINVAL)?;
        check(image.content_serial().is_none())?;
        check(image.configuration().is_some())?;
        for y in 0..480 {
            row.fill(0xff);
            image.read_row(y, &mut row)?;
            check(row == [0; 2560])?;
        }
        fixture.select(&fb, false, 0)?;
        let resumed = compose::current(output, &pool)?.ok_or(EINVAL)?;
        check(resumed.content_serial().is_some() && resumed.content_serial() != visible_serial)?;
        resumed.read_row(0, &mut row)?;
        check(row == [0x73; 2560])?;
        image.read_row(0, &mut row)?;
        check(row == [0; 2560])?;
        Ok(())
    }

    #[test]
    fn blank_output_does_not_inherit_a_removed_producer_error() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        let mut failed = ManualFence::new()?;
        failed.complete(Err(EIO))?;
        fixture.select_with_producer(&fb, false, 0, Some(&failed.fence()))?;
        let pool = pool(&fixture, 640, 480)?;
        let output = &fixture.drm.device().output;
        check(matches!(compose::current(output, &pool), Err(EIO)))?;
        blank(&fixture)?;
        drop(fb);
        let image = compose::current(output, &pool)?.ok_or(EINVAL)?;
        check(image.content_serial().is_none())?;
        let mut row = [0xff; 2560];
        image.read_row(0, &mut row)?;
        check(row == [0; 2560])?;
        Ok(())
    }

    #[test]
    fn blank_results_keep_pool_limits_without_holding_source_admission() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        blank(&fixture)?;
        let output = &fixture.drm.device().output;
        let pool = pool(&fixture, 640, 480)?;
        let first = compose::current(output, &pool)?.ok_or(EINVAL)?;
        let _second = compose::current(output, &pool)?.ok_or(EINVAL)?;
        check(matches!(compose::current(output, &pool), Err(EBUSY)))?;
        let source: ARef<Source> = output
            .inspect_accepted(|accepted| accepted.map(|(source, _)| source.into()))
            .ok_or(EINVAL)?;
        check(source.hold_admission()?.prepared()?.is_some())?;
        fixture.select(&fb, false, 0)?;
        check(source.prepared()?.is_some())?;
        drop(first);
        check(compose::current(output, &pool)?.is_some())?;
        Ok(())
    }

    #[test]
    fn blank_output_rejects_a_pool_with_wrong_dimensions() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        blank(&fixture)?;
        let pool = pool(&fixture, 320, 240)?;
        check(matches!(
            compose::current(&fixture.drm.device().output, &pool),
            Err(EINVAL)
        ))?;
        let _first = pool.reserve()?;
        let _second = pool.reserve()?;
        Ok(())
    }
}
