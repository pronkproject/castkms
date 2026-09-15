// SPDX-License-Identifier: GPL-2.0-only

//! Native host-layout qualification without mapping or reading display pixels.

use super::*;
use crate::{
    host_compositor::framebuffer::Framebuffer as HostFramebuffer,
    scene::Geometry, //
};

fn geometry(width: u32, height: u32) -> Geometry {
    Geometry {
        source: [0, 0, width << 16, height << 16],
        destination: [width, height],
        output: [width, height],
    }
}

fn native(
    fixture: &Fixture,
    size: usize,
    width: u32,
    height: u32,
    pitch: u32,
    offset: u32,
    interlaced: bool,
) -> Result<FramebufferRef<Driver>> {
    let object = shmem::Object::<gem::Object>::new(
        fixture.drm.device(),
        size,
        Default::default(),
        Default::default(),
    )?;
    fixture.drm.framebuffer(
        &FramebufferLayout {
            width,
            height,
            format: drm::fourcc::XRGB8888,
            modifier: Some(drm::fourcc::FORMAT_MOD_LINEAR),
            interlaced,
            planes: &[FramebufferPlane {
                object: &object,
                pitch,
                offset,
            }],
        },
        provenance::Provenance::from_snapshot(None),
    )
}

#[kunit_tests(rust_castkms_host_framebuffers)]
mod cases {
    use super::*;

    #[test]
    fn native_linear_storage_is_retained_without_mapping() -> Result {
        let fixture = Fixture::new()?;
        let fb = native(&fixture, 4096, 64, 2, 260, 4, false)?;
        let host = HostFramebuffer::new(&fb, geometry(64, 2))?;
        drop(fb);
        check(host.dimensions() == (64, 2))?;
        Ok(())
    }

    #[test]
    fn sampling_requires_the_entire_framebuffer() -> Result {
        let fixture = Fixture::new()?;
        let fb = native(&fixture, 4096, 64, 2, 256, 0, false)?;
        let mut crop = geometry(64, 2);
        crop.source[0] = 1 << 16;
        check(matches!(HostFramebuffer::new(&fb, crop), Err(EINVAL)))?;
        let mut scale = geometry(64, 2);
        scale.destination[0] = 32;
        check(matches!(HostFramebuffer::new(&fb, scale), Err(EINVAL)))?;
        let mut clipped = geometry(64, 2);
        clipped.output[0] = 32;
        check(matches!(HostFramebuffer::new(&fb, clipped), Err(EINVAL)))?;
        Ok(())
    }

    #[test]
    fn interlaced_storage_is_not_host_eligible() -> Result {
        let fixture = Fixture::new()?;
        let fb = native(&fixture, 4096, 64, 2, 256, 0, true)?;
        check(fb.is_interlaced())?;
        check(matches!(
            HostFramebuffer::new(&fb, geometry(64, 2)),
            Err(EINVAL)
        ))?;
        Ok(())
    }

    #[test]
    fn host_layout_requires_aligned_complete_rows() -> Result {
        let fixture = Fixture::new()?;
        let unaligned = native(&fixture, 4096, 64, 2, 257, 0, false)?;
        check(matches!(
            HostFramebuffer::new(&unaligned, geometry(64, 2)),
            Err(EINVAL)
        ))?;
        let short_padding = native(&fixture, 8192, 64, 2, 4096, 4, false)?;
        check(matches!(
            HostFramebuffer::new(&short_padding, geometry(64, 2)),
            Err(EINVAL)
        ))?;
        Ok(())
    }

    #[test]
    fn source_allocation_is_bounded_independently_of_image_size() -> Result {
        let fixture = Fixture::new()?;
        let oversized = native(&fixture, execution::host::MAX_ALLOCATION_BYTES + 4096, 64, 2, 256, 0, false)?;
        check(matches!(
            HostFramebuffer::new(&oversized, geometry(64, 2)),
            Err(E2BIG)
        ))?;
        Ok(())
    }

    #[test]
    fn driver_bounds_match_the_host_dimensions() -> Result {
        let fixture = Fixture::new()?;
        let uhd = native(&fixture, 3840 * 2160 * 4, 3840, 2160, 15360, 0, false)?;
        check(HostFramebuffer::new(&uhd, geometry(3840, 2160))?.dimensions() == (3840, 2160))?;
        check(matches!(
            native(&fixture, 4096, 8193, 1, 32772, 0, false),
            Err(EINVAL)
        ))?;
        check(matches!(
            native(&fixture, 4096, 1, 8193, 4, 0, false),
            Err(EINVAL)
        ))?;
        Ok(())
    }

    #[cfg(CONFIG_DRM_CLIENT)]
    #[test]
    fn imported_linear_storage_is_not_host_eligible() -> Result {
        with_exporter(|source| {
            let buffer = source.drm.export_dumb(64, 64, 32)?;
            let target = CastKms::new(c"castkms-host-import-test")?;
            let object = {
                let registered = target._display.registration_guard().ok_or(ENODEV)?;
                shmem::Object::<gem::Object>::import(&registered, &buffer)?
            };
            let fb = {
                let registered = target._display.registration_guard().ok_or(ENODEV)?;
                kernel::drm::kms::framebuffer::Framebuffer::from_objects(
                    &registered,
                    &FramebufferLayout {
                        width: 64,
                        height: 64,
                        format: drm::fourcc::XRGB8888,
                        modifier: Some(drm::fourcc::FORMAT_MOD_LINEAR),
                        interlaced: false,
                        planes: &[FramebufferPlane {
                            object: &object,
                            pitch: 256,
                            offset: 0,
                        }],
                    },
                )?
            };
            check(matches!(
                HostFramebuffer::new(&fb, geometry(64, 64)),
                Err(EOPNOTSUPP)
            ))?;
            Ok(())
        })
    }
}
