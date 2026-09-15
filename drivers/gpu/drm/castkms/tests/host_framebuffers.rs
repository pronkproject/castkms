// SPDX-License-Identifier: GPL-2.0-only

//! Native host-layout qualification without mapping or reading display pixels.

use super::*;
use crate::{
    host_compositor::framebuffer::Framebuffer as HostFramebuffer,
    scene::Geometry, //
};

fn geometry(width: u32, height: u32) -> Geometry {
    Geometry {
        position: [0, 0],
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
    native_format(
        fixture,
        size,
        width,
        height,
        pitch,
        offset,
        interlaced,
        drm::fourcc::XRGB8888,
    )
}

fn native_format(
    fixture: &Fixture,
    size: usize,
    width: u32,
    height: u32,
    pitch: u32,
    offset: u32,
    interlaced: bool,
    format: u32,
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
            format,
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
    fn sampling_accepts_cropping_scaling_and_clipping() -> Result {
        let fixture = Fixture::new()?;
        let fb = native(&fixture, 4096, 64, 2, 256, 0, false)?;
        let mut crop = geometry(64, 2);
        crop.source[0] = 1 << 16;
        crop.source[2] = 63 << 16;
        HostFramebuffer::new(&fb, crop)?;
        let mut scale = geometry(64, 2);
        scale.destination[0] = 32;
        HostFramebuffer::new(&fb, scale)?;
        let mut clipped = geometry(64, 2);
        clipped.output[0] = 32;
        HostFramebuffer::new(&fb, clipped)?;
        crop.source[2] = 64 << 16;
        check(matches!(HostFramebuffer::new(&fb, crop), Err(EINVAL)))?;
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
    fn byte_reads_allow_unaligned_rows_without_final_padding() -> Result {
        let fixture = Fixture::new()?;
        let unaligned = native(&fixture, 4096, 64, 2, 257, 0, false)?;
        HostFramebuffer::new(&unaligned, geometry(64, 2))?;
        let short_padding = native(&fixture, 8192, 64, 2, 4096, 4, false)?;
        HostFramebuffer::new(&short_padding, geometry(64, 2))?;
        Ok(())
    }

    #[test]
    fn source_allocation_is_bounded_independently_of_image_size() -> Result {
        let fixture = Fixture::new()?;
        let oversized = native(
            &fixture,
            execution::host::MAX_ALLOCATION_BYTES + 4096,
            64,
            2,
            256,
            0,
            false,
        )?;
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

    #[test]
    fn single_plane_formats_are_host_eligible() -> Result {
        let fixture = Fixture::new()?;
        for &format in execution::host::FORMATS {
            if crate::formats::plane_count(format) != 1 { continue; }
            let pitch = crate::formats::plane(format, 0)?.row_bytes(8) as u32;
            let fb = native_format(&fixture, 4096, 8, 2, pitch, 0, false, format)?;
            check(HostFramebuffer::new(&fb, geometry(8, 2))?.dimensions() == (8, 2))?;
        }
        Ok(())
    }

    #[test]
    fn packed_rgb_pixels_are_normalized_to_xrgb8888() -> Result {
        use drm::fourcc;

        let pixel = |format, value: u32| crate::formats::pixel(format, 0, 0, |_, _, _, out| {
            out.copy_from_slice(&value.to_le_bytes()[..out.len()]);
            Ok(())
        });

        check(pixel(fourcc::ARGB8888, 0xaa12_3456)? == 0x0012_3456)?;
        check(pixel(fourcc::ABGR8888, 0xaa12_3456)? == 0x0056_3412)?;
        check(pixel(fourcc::XRGB2101010, 0x3ff0_0000)? == 0x00ff_0000)?;
        check(pixel(fourcc::XBGR2101010, 0x0000_03ff)? == 0x00ff_0000)?;
        check(matches!(
            pixel(0, 0),
            Err(EINVAL)
        ))
    }

    #[cfg(CONFIG_DRM_CLIENT)]
    #[test]
    fn imported_linear_storage_is_host_eligible() -> Result {
        with_exporter(|source| {
            let buffer = source.drm.export_dumb(64, 64, 32)?;
            {
                let mut write = kernel::dma_buf::cpu_access::Write::new(&buffer)?;
                write.copy_from_slice(256 + 12, &[0x12, 0x34, 0x56, 0x78])?;
                write.finish()?;
            }
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
            let host = HostFramebuffer::new(&fb, geometry(64, 64))?;
            let mut bytes = [0; 4];
            host.read_for_test(0, 12, 1, &mut bytes)?;
            check(bytes == [0x12, 0x34, 0x56, 0x78])?;
            Ok(())
        })
    }
}
