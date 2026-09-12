// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::host_compositor::{
    compose,
    pool::Pool, //
};
use kernel::{
    drm::preparation::Source,
    io::{
        io_project,
        Io, //
    },
    sync::aref::ARef, //
};

#[kunit_tests(rust_castkms_host_composition)]
mod cases {
    use super::*;

    #[test]
    fn completed_pixels_do_not_retain_the_source_read() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        {
            let mapping = fb.vmap::<gem::Object>()?;
            io_project!(mapping.view(), [try: 0..2560]).copy_from_slice(&[0x35; 2560]);
        }
        fixture.select(&fb, false, 0)?;
        let output = &fixture.drm.device().output;
        let source: ARef<Source> = output
            .inspect_accepted(|accepted| accepted.map(|(source, _)| source.into()))
            .ok_or(EINVAL)?;
        let serial = output
            .inspect(|scene| scene.map(|scene| scene.content_serial()))
            .ok_or(EINVAL)?;
        let pool = Pool::new(fixture.drm.device(), 640, 480)?;
        let completed = compose::current(output, &pool)?.ok_or(EINVAL)?;
        assert_eq!(completed.content_serial(), serial);
        assert!(completed.owner().is_none());
        assert!(source.hold_admission()?.prepared()?.is_some());
        {
            let mapping = fb.vmap::<gem::Object>()?;
            io_project!(mapping.view(), [try: 0..2560]).copy_from_slice(&[0x71; 2560]);
        }
        fixture.select(&fb, false, 0)?;
        assert!(source.prepared()?.is_some());
        let mut row = [0; 2560];
        completed.read_row(0, &mut row)?;
        assert_eq!(row, [0x35; 2560]);
        let newer = compose::current(output, &pool)?.ok_or(EINVAL)?;
        newer.read_row(0, &mut row)?;
        assert_eq!(row, [0x71; 2560]);
        assert_ne!(newer.content_serial(), completed.content_serial());
        assert!(matches!(compose::current(output, &pool), Err(EBUSY)));
        let current: ARef<Source> = output
            .inspect_accepted(|accepted| accepted.map(|(source, _)| source.into()))
            .ok_or(EINVAL)?;
        assert!(current.hold_admission()?.prepared()?.is_some());
        drop(completed);
        assert!(compose::current(output, &pool)?.is_some());
        Ok(())
    }

    #[test]
    fn source_offsets_and_padding_do_not_enter_the_private_image() -> Result {
        let fixture = Fixture::new()?;
        let object = shmem::Object::<gem::Object>::new(
            fixture.drm.device(),
            2 * 1024 * 1024,
            Default::default(),
            (),
        )?;
        {
            let mapping = object.vmap::<0>()?;
            mapping.try_write32(0xeeeeeeee, 0)?;
        }
        let fb = fixture.drm.framebuffer(
            &FramebufferLayout {
                width: 640,
                height: 480,
                format: drm::fourcc::XRGB8888,
                modifier: Some(drm::fourcc::FORMAT_MOD_LINEAR),
                interlaced: false,
                planes: &[FramebufferPlane {
                    object: &object,
                    pitch: 2564,
                    offset: 4,
                }],
            },
            provenance::Provenance::from_snapshot(None),
        )?;
        {
            let mapping = fb.vmap::<gem::Object>()?;
            io_project!(mapping.view(), [try: 0..2564]).copy_from_slice(&[0x35; 2564]);
            io_project!(mapping.view(), [try: 2564..5128]).copy_from_slice(&[0x71; 2564]);
        }
        fixture.select(&fb, false, 0)?;
        let pool = Pool::new(fixture.drm.device(), 640, 480)?;
        let completed = compose::current(&fixture.drm.device().output, &pool)?.ok_or(EINVAL)?;
        let mut row = [0; 2560];
        completed.read_row(0, &mut row)?;
        assert_eq!(row, [0x35; 2560]);
        completed.read_row(1, &mut row)?;
        assert_eq!(row, [0x71; 2560]);
        completed.read_row(2, &mut row)?;
        assert_eq!(row, [0; 2560]);
        Ok(())
    }

    #[test]
    fn failed_producers_do_not_publish_a_private_image() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        let mut producer = kernel::dma_fence::testing::ManualFence::new()?;
        producer.complete(Err(EIO))?;
        fixture.select_with_producer(&fb, false, 0, Some(&producer.fence()))?;
        let pool = Pool::new(fixture.drm.device(), 640, 480)?;
        assert!(matches!(
            compose::current(&fixture.drm.device().output, &pool),
            Err(EIO)
        ));
        let _first = pool.reserve()?;
        let _second = pool.reserve()?;
        Ok(())
    }

    #[test]
    fn pending_producers_release_resources_before_retry() -> Result {
        use crate::{
            output::SceneUpdate,
            scene::{
                ContentSerial,
                Geometry,
                Scene, //
            }, //
        };
        use kernel::{
            drm::kms::framebuffer::dependencies::Dependencies,
            sync::Arc, //
        };

        for fail in [false, true] {
            let fixture = Fixture::new()?;
            let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
            let mut producer = kernel::dma_fence::testing::ManualFence::new()?;
            let records = Arc::new(
                Dependencies::acquire(&fb, Some(producer.fence()))?,
                GFP_KERNEL,
            )?;
            let scene = Scene::new(
                fb,
                Geometry {
                    source: [0, 0, 640 << 16, 480 << 16],
                    destination: [640, 480],
                    output: [640, 480],
                },
                ContentSerial::for_update(None, true)?.ok_or(EINVAL)?,
                None,
                Some(records),
            );
            let source = Source::new(1)?;
            let output = &fixture.drm.device().output;
            output.publish(source.clone(), SceneUpdate::Replace(Some(scene)));
            let pool = Pool::new(fixture.drm.device(), 640, 480)?;
            assert!(matches!(compose::current(output, &pool), Err(EAGAIN)));
            assert!(source.hold_admission()?.prepared()?.is_some());
            {
                let _first = pool.reserve()?;
                let _second = pool.reserve()?;
            }
            producer.complete(if fail { Err(EIO) } else { Ok(()) })?;
            if fail {
                assert!(matches!(compose::current(output, &pool), Err(EIO)));
            } else {
                assert!(compose::current(output, &pool)?.is_some());
            }
            assert!(source.hold_admission()?.prepared()?.is_some());
        }
        Ok(())
    }

    #[test]
    fn missing_or_mismatched_images_release_the_reserved_slot() -> Result {
        let fixture = Fixture::new()?;
        let pool = Pool::new(fixture.drm.device(), 3, 2)?;
        let output = &fixture.drm.device().output;
        assert!(compose::current(output, &pool)?.is_none());
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        assert!(matches!(compose::current(output, &pool), Err(EINVAL)));
        let _first = pool.reserve()?;
        let _second = pool.reserve()?;
        Ok(())
    }
}
