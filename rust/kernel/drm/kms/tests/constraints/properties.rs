// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Standard property identities feed native proposed-state constraints checks.

use super::*;
use crate::drm::constraints::Property;
use plane::{ColorEncoding, ColorRange, SceneProperty};

#[kunit_tests(rust_drm_constraints_properties)]
mod cases {
    use super::*;

    #[test]
    fn attached_ids_describe_color_and_stacking_rules() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        counts.constraints_capacity.store(4, Ordering::Relaxed);
        counts.scene_properties.store(1, Ordering::Relaxed);
        let parent = faux::Registration::new(c"rust-constraints-scene-properties", None)?;
        let dev = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let output = dev.constraints_output(0)?;
        let plane = dev.plane()?;
        let id = plane.object_id();
        let zpos = plane.scene_property_id(SceneProperty::Zpos).ok_or(EINVAL)?;
        let encoding = plane
            .scene_property_id(SceneProperty::ColorEncoding)
            .ok_or(EINVAL)?;
        let range = plane
            .scene_property_id(SceneProperty::ColorRange)
            .ok_or(EINVAL)?;
        let crtc_width = plane
            .scene_property_id(SceneProperty::CrtcWidth)
            .ok_or(EINVAL)?;
        let source_width = plane
            .scene_property_id(SceneProperty::SourceWidth)
            .ok_or(EINVAL)?;
        assert_eq!(range, counts.scene_color_range_id.load(Ordering::Relaxed));
        assert_ne!(encoding, range);
        assert_ne!(crtc_width, source_width);
        assert_eq!(plane.scene_property_id(SceneProperty::Alpha), None);
        assert_eq!(plane.scene_property_id(SceneProperty::Rotation), None);
        assert!(plane
            .scene_property_id(SceneProperty::ScalingFilter)
            .is_some());
        let size = Size::exact(640, 480);
        let description = Description::new(
            size,
            &[Format::new(
                id,
                fourcc::NV12,
                fourcc::FORMAT_MOD_LINEAR,
                size,
            )],
            &[
                Property::unsigned_range(id, zpos, 2, 3),
                Property::enum_values(
                    id,
                    encoding,
                    1 << bindings::drm_color_encoding_DRM_COLOR_YCBCR_BT709,
                ),
                Property::enum_values(
                    id,
                    range,
                    1 << bindings::drm_color_range_DRM_COLOR_YCBCR_LIMITED_RANGE,
                ),
                Property::unsigned_range(id, crtc_width, 640, 640),
                Property::unsigned_range(id, source_width, 640 << 16, 640 << 16),
            ],
        )?;
        let target =
            OpaqueEntry::new_stateless(output.domain(), dev.crtc()?.object_id(), &description)?;
        output.add(&target)?;
        let image = provider::nv12(&dev, &counts)?;
        let mode = mode()?;
        let scanout = atomic::CrtcScanout {
            mode: &mode,
            framebuffer: &image,
            connectors: &[dev.connector()?],
            position: (0, 0),
        };
        dev.update(|mut state| {
            state
                .as_mut()
                .set_crtc_config(dev.crtc()?, Some(&scanout))?;
            state
                .add_crtc_state(dev.crtc()?)?
                .set_constraints(&target)?;
            let mut new = state.add_plane_state(plane)?;
            new.set_zpos(2)?;
            new.set_yuv_color(ColorEncoding::Bt709, ColorRange::Limited)
        })?;
        assert_eq!(output.selected().id(), target.id());
        let generation = output.snapshot(0)?.info().generation;
        assert_eq!(
            dev.update(|state| state.add_plane_state(plane)?.set_zpos(1)),
            Err(EINVAL)
        );
        assert_eq!(
            dev.update(|state| state
                .add_plane_state(plane)?
                .set_yuv_color(ColorEncoding::Bt601, ColorRange::Limited,)),
            Err(EINVAL)
        );
        assert_eq!(
            dev.update(|state| state
                .add_plane_state(plane)?
                .set_yuv_color(ColorEncoding::Bt709, ColorRange::Full,)),
            Err(EINVAL)
        );
        assert_eq!(output.selected().id(), target.id());
        assert_eq!(output.snapshot(0)?.info().generation, generation);
        output.close();
        dev.update(|state| state.set_crtc_config(dev.crtc()?, None))?;
        drop(image);
        drop(output);
        drop(dev);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }
}
