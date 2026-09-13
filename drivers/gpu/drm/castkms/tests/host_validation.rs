// SPDX-License-Identifier: GPL-2.0-only

//! Host storage limits at atomic validation, before replacing a published scene.

use super::*;

fn framebuffer(fixture: &Fixture, size: usize, pitch: u32) -> Result<FramebufferRef<Driver>> {
    let size = kernel::page::page_align(size).ok_or(EOVERFLOW)?;
    let object =
        shmem::Object::<gem::Object>::new(fixture.drm.device(), size, Default::default(), ())?;
    fixture.drm.framebuffer(
        &FramebufferLayout {
            width: 640,
            height: 480,
            format: drm::fourcc::XRGB8888,
            modifier: Some(drm::fourcc::FORMAT_MOD_LINEAR),
            interlaced: false,
            planes: &[FramebufferPlane {
                object: &object,
                pitch,
                offset: 0,
            }],
        },
        provenance::Provenance::from_snapshot(None),
    )
}

#[kunit_tests(rust_castkms_host_validation)]
mod cases {
    use super::*;

    #[test]
    fn oversized_storage_cannot_replace_the_host_scene() -> Result {
        let fixture = Fixture::new()?;
        let valid = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&valid, false, 0)?;
        let invalid = framebuffer(&fixture, 16 * 1024 * 1024 + 4096, 2560)?;
        for check_only in [true, false] {
            check(fixture.select(&invalid, check_only, 0) == Err(E2BIG))?;
            check(fixture.drm.device().output.inspect(|scene| {
                scene
                    .and_then(|scene| scene.primary())
                    .is_some_and(|primary| core::ptr::eq(primary.framebuffer(), &*valid))
            }))?;
        }
        Ok(())
    }

    #[test]
    fn unaligned_rows_fail_before_a_source_is_published() -> Result {
        let fixture = Fixture::new()?;
        let invalid = framebuffer(&fixture, 2561 * 480, 2561)?;
        for check_only in [true, false] {
            check(fixture.select(&invalid, check_only, 0) == Err(EINVAL))?;
            check(!fixture.drm.device().output.has_scene())?;
        }
        Ok(())
    }
}
