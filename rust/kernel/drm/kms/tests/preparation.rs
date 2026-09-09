// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::{dma_fence::testing::ManualFence, dma_resv::Usage};

#[kunit_tests(rust_drm_framebuffer_preparation)]
mod cases {
    use super::*;

    #[test]
    fn failed_preparation_does_not_publish_and_can_retry() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-prepare-failure", None)?;
        let dev = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let fb = framebuffer(dev.device())?;
        let mode = mode()?;
        let scanout = atomic::CrtcScanout {
            mode: &mode,
            framebuffer: &fb,
            connectors: &[dev.connector()?],
            position: (0, 0),
        };
        counts
            .fail_framebuffer_preparation
            .store(1, Ordering::Relaxed);
        dev.check(|state| state.set_crtc_config(dev.crtc()?, Some(&scanout)))?;
        assert_eq!(counts.framebuffer_preparations.load(Ordering::Relaxed), 0);
        assert_eq!(
            dev.update(|state| state.set_crtc_config(dev.crtc()?, Some(&scanout))),
            Err(ENOMEM)
        );
        assert_eq!(counts.framebuffer_preparations.load(Ordering::Relaxed), 1);
        assert_eq!(counts.plane_updates.load(Ordering::Relaxed), 0);
        counts
            .fail_framebuffer_preparation
            .store(0, Ordering::Relaxed);
        dev.update(|state| state.set_crtc_config(dev.crtc()?, Some(&scanout)))?;
        assert_eq!(counts.plane_updates.load(Ordering::Relaxed), 1);
        drop(fb);
        drop(dev);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.plane_states.load(Ordering::Relaxed), 0);
        Ok(())
    }

    fn memory_planes(alias: bool) -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-prepare-planes", None)?;
        let dev = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let first =
            gem::shmem::Object::<TestObject>::new(dev.device(), 4096, Default::default(), ())?;
        let second = if alias {
            first.clone()
        } else {
            gem::shmem::Object::<TestObject>::new(dev.device(), 4096, Default::default(), ())?
        };
        let fb = dev.framebuffer(
            &framebuffer::FramebufferLayout {
                width: 16,
                height: 16,
                format: fourcc::NV12,
                modifier: None,
                interlaced: false,
                planes: &[
                    framebuffer::FramebufferPlane {
                        object: &first,
                        pitch: 16,
                        offset: 0,
                    },
                    framebuffer::FramebufferPlane {
                        object: &second,
                        pitch: 16,
                        offset: 256,
                    },
                ],
            },
            framebuffers::Metadata::new(&counts, false),
        )?;
        assert_eq!(fb.plane_count(), 2);
        assert!(fb.object_at(2).is_err());
        let mut first_fence = ManualFence::new()?;
        let mut second_fence = ManualFence::new()?;
        dev.add_framebuffer_fence(&fb, 0, &first_fence.fence(), Usage::Write)?;
        if !alias {
            dev.add_framebuffer_fence(&fb, 1, &second_fence.fence(), Usage::Write)?;
        }
        let records = framebuffer::dependencies::Dependencies::acquire(&fb, None)?;
        assert_eq!(records.iter().len(), if alias { 1 } else { 2 });
        drop(fb);
        drop(first);
        drop(second);
        drop(dev);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        first_fence.complete(Err(EIO))?;
        second_fence.complete(Err(EIO))?;
        assert!(records
            .iter()
            .all(|fence| fence.status() == crate::dma_fence::Status::Complete(Err(EIO))));
        Ok(())
    }

    #[test]
    fn every_memory_plane_contributes_dependencies() -> Result {
        memory_planes(false)
    }

    #[test]
    fn aliased_memory_planes_sample_one_reservation() -> Result {
        memory_planes(true)
    }
}
