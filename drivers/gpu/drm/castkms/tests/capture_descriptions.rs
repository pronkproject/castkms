// SPDX-License-Identifier: GPL-2.0-only

//! Layout negotiation remains tied to its grant and the accepted display configuration.

mod negotiation;

use super::*;
use crate::capture::{
    host_stream::Stream,
    provider::Grantor, //
};
use kernel::{
    drm::{
        fourcc,
        kms::testing::MasterFile, //
    },
    io::{
        io_project,
        Io, //
    }, //
};

fn grant(fixture: &Fixture, file: &MasterFile<'_, Driver>) -> Result<Grantor> {
    File::create_capture_grant(file.file(), fixture.drm.crtc()?, fixture.drm.connector()?)
}

fn select(fixture: &Fixture, file: &MasterFile<'_, Driver>) -> Result<FramebufferRef<Driver>> {
    let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(
        file.file().master_snapshot(),
    ))?;
    fixture.select(&fb, false, 0)?;
    Ok(fb)
}

#[kunit_tests(rust_castkms_capture_descriptions)]
mod cases {
    use super::*;

    #[test]
    fn described_layout_matches_the_host_capture_result() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let description = grantor.capture().describe_stream()?;
        check(description.format() == fourcc::XRGB8888)?;
        check(description.modifier() == fourcc::FORMAT_MOD_LINEAR)?;
        check(description.layout().dimensions() == (640, 480))?;
        check(description.layout().pitch() == 2560)?;
        check(description.max_requests() == 8)?;
        let mut stream = Stream::from_description(&description, 2)?;
        let result = stream.capture()?;
        let mut pixels = KVVec::new();
        pixels.resize(description.layout().pixel_bytes(), 0x83, GFP_KERNEL)?;
        check(result.copy_result(&mut pixels)? == pixels.len())?;
        Ok(())
    }

    #[test]
    fn descriptions_do_not_reserve_stream_credit() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let capture = grantor.capture();
        let description = capture.describe_stream()?;
        let mut streams = KVec::new();
        for _ in 0..16 {
            streams.push(description.create_stream(1)?, GFP_KERNEL)?;
        }
        check(matches!(description.create_stream(1), Err(EBUSY)))?;
        let mut descriptions = KVec::new();
        for _ in 0..32 {
            descriptions.push(capture.describe_stream()?, GFP_KERNEL)?;
        }
        check(matches!(description.create_stream(1), Err(EBUSY)))?;
        drop(streams.pop());
        let _replacement = descriptions[0].create_stream(1)?;
        Ok(())
    }

    #[test]
    fn content_updates_preserve_the_described_layout_without_freezing_pixels() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let description = grantor.capture().describe_stream()?;
        {
            let mapping = fb.vmap::<gem::Object>()?;
            io_project!(mapping.view(), [try: 0..2560]).copy_from_slice(&[0x39; 2560]);
        }
        fixture.select(&fb, false, 0)?;
        let mut stream = Stream::from_description(&description, 1)?;
        let result = stream.capture()?;
        let mut pixels = KVVec::new();
        pixels.resize(description.layout().pixel_bytes(), 0, GFP_KERNEL)?;
        result.copy_result(&mut pixels)?;
        check(pixels[..4] == [0x39, 0x39, 0x39, 0xff])?;
        Ok(())
    }

    #[test]
    fn a_same_size_modeset_requires_a_fresh_description() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let capture = grantor.capture();
        let old = capture.describe_stream()?;
        fixture.drm.update(|transaction| {
            transaction
                .add_crtc_state(fixture.drm.crtc()?)?
                .set_mode_changed(true);
            Ok(())
        })?;
        for _ in 0..32 {
            check(matches!(old.create_stream(1), Err(ESTALE)))?;
        }
        check(matches!(Stream::from_description(&old, 1), Err(ESTALE)))?;
        let fresh = capture.describe_stream()?;
        check(fresh.layout() == old.layout())?;
        let mut streams = KVec::new();
        for _ in 0..16 {
            streams.push(fresh.create_stream(1)?, GFP_KERNEL)?;
        }
        Ok(())
    }

    #[test]
    fn descriptions_do_not_retain_the_grantor() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let capture = grantor.capture();
        let description = capture.describe_stream()?;
        drop(grantor);
        check(matches!(capture.describe_stream(), Err(EKEYREVOKED)))?;
        check(matches!(description.create_stream(1), Err(EKEYREVOKED)))?;
        check(matches!(
            Stream::from_description(&description, 1),
            Err(EKEYREVOKED)
        ))?;
        Ok(())
    }

    #[test]
    fn creator_close_revokes_retained_descriptions() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let description = grantor.capture().describe_stream()?;
        drop(file);
        check(matches!(description.create_stream(1), Err(EKEYREVOKED)))?;
        Ok(())
    }

    #[test]
    fn independent_descriptions_remain_bound_to_their_own_grants() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = select(&fixture, &file)?;
        let first = grant(&fixture, &file)?;
        let second = grant(&fixture, &file)?;
        let first_description = first.capture().describe_stream()?;
        let second_description = second.capture().describe_stream()?;
        drop(first);
        check(matches!(
            first_description.create_stream(1),
            Err(EKEYREVOKED)
        ))?;
        let mut stream = Stream::from_description(&second_description, 1)?;
        check(stream.capture()?.wait()? == Ok(()))?;
        Ok(())
    }

    #[test]
    fn inactive_and_reenabled_outputs_do_not_reuse_a_description() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let grantor = grant(&fixture, &file)?;
        let capture = grantor.capture();
        check(matches!(capture.describe_stream(), Err(ENODEV)))?;
        let fb = select(&fixture, &file)?;
        let old = capture.describe_stream()?;
        fixture.drm.update(|mut transaction| {
            transaction
                .as_mut()
                .set_crtc_config(fixture.drm.crtc()?, None)
        })?;
        check(matches!(old.create_stream(1), Err(ENODEV)))?;
        fixture.select(&fb, false, 0)?;
        check(matches!(old.create_stream(1), Err(ESTALE)))?;
        let fresh = capture.describe_stream()?;
        let _stream = fresh.create_stream(1)?;
        fixture.state.close();
        check(matches!(fresh.create_stream(1), Err(EKEYREVOKED)))?;
        check(matches!(capture.describe_stream(), Err(EKEYREVOKED)))?;
        Ok(())
    }
}
