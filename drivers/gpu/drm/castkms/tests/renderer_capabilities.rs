// SPDX-License-Identifier: GPL-2.0-only

//! Whole-scene matching against retained native framebuffer metadata.

use super::*;
use crate::execution::capabilities::{ColorLimits, Format, GeometryLimits, Limits, Profile};
use kernel::drm::kms::{
    colorop::Operation,
    crtc::{ColorCtm, ColorLut},
};

fn limits() -> Limits {
    Limits {
        geometry: GeometryLimits {
            min_output: [1; 2],
            min_source: [1; 2],
            output: [16384; 2],
            source: [16384; 2],
            crop: true,
            fractional: true,
            position: true,
            scale: true,
            min_scale: 1 << 12,
            max_scale: 1 << 20,
        },
        color: ColorLimits {
            operations: 16,
            srgb: true,
            plane_matrix: true,
            output_matrix: true,
            lut_entries: 256,
            yuv_encodings: [false; 3],
            yuv_ranges: [false; 2],
        },
        layers: 24,
        roles: [1, 22, 1],
    }
}

fn profile(limits: Limits, image: &FramebufferRef<Driver>) -> Result<Profile> {
    let mut formats = KVec::new();
    formats.push(
        Format {
            fourcc: image.format(),
            modifier: image.modifier(),
            planes: image.plane_count() as u32,
            native: true,
            imported: false,
            pitch_alignment: 1,
            offset_alignment: 1,
            max_pitch: u32::MAX,
        },
        GFP_KERNEL,
    )?;
    Profile::new(limits, formats)
}

fn scene(fixture: &Fixture, image: &FramebufferRef<Driver>) -> Result<scene::Scene> {
    fixture.select(image, false, 0)?;
    fixture
        .drm
        .device()
        .output
        .inspect(|scene| scene.cloned())
        .ok_or(EINVAL)
}

#[kunit_tests(rust_castkms_renderer_capabilities)]
mod cases {
    use super::*;

    #[test]
    fn every_layer_must_fit_the_profile() -> Result {
        let fixture = Fixture::new()?;
        let image = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        let mut scene = scene(&fixture, &image)?;
        check(!image.is_yuv())?;
        let mut limits = limits();
        profile(limits, &image)?.check(&scene, [640, 480])?;
        let mut overlay = scene.primary().ok_or(EINVAL)?.clone();
        overlay.kind = scene::Kind::Overlay;
        scene.set_layer(1, Some(Arc::new(overlay.clone(), GFP_KERNEL)?));
        profile(limits, &image)?.check(&scene, [640, 480])?;
        limits.roles[1] = 0;
        check(profile(limits, &image)?.check(&scene, [640, 480]) == Err(EOPNOTSUPP))?;
        limits.roles[1] = 22;
        limits.layers = 1;
        limits.roles = [1; 3];
        check(profile(limits, &image)?.check(&scene, [640, 480]) == Err(EOPNOTSUPP))?;
        overlay.geometry.output = [641, 480];
        scene.set_layer(1, Some(Arc::new(overlay, GFP_KERNEL)?));
        check(profile(super::limits(), &image)?.check(&scene, [640, 480]) == Err(EOPNOTSUPP))?;
        Ok(())
    }

    #[test]
    fn plane_and_output_colors_have_independent_limits() -> Result {
        let fixture = Fixture::new()?;
        let image = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        let mut scene = scene(&fixture, &image)?;
        let mut layer = scene.primary().ok_or(EINVAL)?.clone();
        let mut operations = KVec::new();
        operations.push(Operation::SrgbEotf, GFP_KERNEL)?;
        operations.push(Operation::Matrix([0; 12]), GFP_KERNEL)?;
        layer.color = crate::color::Pipeline::new(operations)?;
        scene.set_layer(0, Some(Arc::new(layer, GFP_KERNEL)?));
        let mut limits = limits();
        profile(limits, &image)?.check(&scene, [640, 480])?;
        for color in [
            ColorLimits {
                srgb: false,
                ..limits.color
            },
            ColorLimits {
                plane_matrix: false,
                ..limits.color
            },
            ColorLimits {
                operations: 1,
                ..limits.color
            },
        ] {
            check(
                profile(Limits { color, ..limits }, &image)?.check(&scene, [640, 480])
                    == Err(EOPNOTSUPP),
            )?;
        }
        scene.output_color = crate::color::OutputColor::new(
            Some(&[ColorLut::new(0, 0, 0), ColorLut::new(65535, 65535, 65535)]),
            None,
            None,
        )?;
        profile(limits, &image)?.check(&scene, [640, 480])?;
        limits.color.lut_entries = 1;
        check(profile(limits, &image)?.check(&scene, [640, 480]) == Err(EOPNOTSUPP))?;
        let mut blank = scene::Scene::blank(None);
        profile(limits, &image)?.check(&blank, [640, 480])?;
        blank.output_color = scene.output_color;
        check(profile(limits, &image)?.check(&blank, [640, 480]) == Err(EOPNOTSUPP))?;
        blank.output_color =
            crate::color::OutputColor::new(None, Some(&ColorCtm::from_raw([0; 9])), None)?;
        profile(limits, &image)?.check(&blank, [640, 480])?;
        limits.color.output_matrix = false;
        check(profile(limits, &image)?.check(&blank, [640, 480]) == Err(EOPNOTSUPP))?;
        Ok(())
    }

    #[test]
    fn yuv_requires_color_conversion_support() -> Result {
        let fixture = Fixture::new()?;
        let rgb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        let mut scene = scene(&fixture, &rgb)?;
        let object = rgb.object_at(0)?;
        let image = fixture.drm.framebuffer(
            &FramebufferLayout {
                width: 2,
                height: 2,
                format: drm::fourcc::NV12,
                modifier: Some(drm::fourcc::FORMAT_MOD_LINEAR),
                interlaced: false,
                planes: &[
                    FramebufferPlane {
                        object,
                        pitch: 2,
                        offset: 0,
                    },
                    FramebufferPlane {
                        object,
                        pitch: 2,
                        offset: 4,
                    },
                ],
            },
            provenance::Provenance::from_snapshot(None),
        )?;
        check(image.is_yuv())?;
        let mut layer = scene.primary().ok_or(EINVAL)?.clone();
        layer.framebuffer = image.clone();
        layer.geometry.source = [0, 0, 2 << 16, 2 << 16];
        layer.geometry.destination = [2; 2];
        scene.set_layer(0, Some(Arc::new(layer, GFP_KERNEL)?));
        let mut limits = limits();
        check(profile(limits, &image)?.check(&scene, [640, 480]) == Err(EOPNOTSUPP))?;
        limits.color.yuv_encodings = [true; 3];
        check(profile(limits, &image)?.check(&scene, [640, 480]) == Err(EOPNOTSUPP))?;
        limits.color.yuv_ranges = [true; 2];
        profile(limits, &image)?.check(&scene, [640, 480])?;
        Ok(())
    }

    #[test]
    fn exact_tiled_profile_is_independent_of_host_layouts() -> Result {
        let fixture = Fixture::new()?;
        let linear = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        let object = linear.object_at(0)?;
        let tiled = fixture.drm.framebuffer(
            &FramebufferLayout {
                width: 640,
                height: 480,
                format: drm::fourcc::XRGB8888,
                modifier: Some(drm::fourcc::I915_FORMAT_MOD_4_TILED),
                interlaced: false,
                planes: &[FramebufferPlane {
                    object,
                    pitch: 640 * 4,
                    offset: 0,
                }],
            },
            provenance::Provenance::from_snapshot(None),
        )?;
        let mut scene = scene(&fixture, &linear)?;
        let mut layer = scene.primary().ok_or(EINVAL)?.clone();

        layer.framebuffer = tiled.clone();
        scene.set_layer(0, Some(Arc::new(layer, GFP_KERNEL)?));
        profile(limits(), &tiled)?.check(&scene, [640, 480])?;
        check(profile(limits(), &linear)?.check(&scene, [640, 480]) == Err(EOPNOTSUPP))?;
        check(
            execution::host::check_framebuffer(
                &tiled,
                scene.primary().ok_or(EINVAL)?.geometry(),
            ) == Err(EINVAL),
        )?;
        Ok(())
    }
}
