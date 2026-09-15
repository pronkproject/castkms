// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use capabilities::{ColorLimits, Format, GeometryLimits, Limits, Profile};
use kernel::drm::fourcc;

fn renderer(output: [u32; 2]) -> Result<Contract> {
    let mut formats = KVec::new();
    formats.push(
        Format {
            fourcc: fourcc::XRGB8888,
            modifier: None,
            planes: 1,
            native: true,
            imported: true,
            pitch_alignment: 4,
            offset_alignment: 4,
            max_pitch: 1 << 20,
        },
        GFP_KERNEL,
    )?;
    Ok(Contract::Renderer(Arc::new(
        Profile::new(
            Limits {
                geometry: GeometryLimits {
                    output,
                    source: output,
                    crop: true,
                    fractional: true,
                    position: true,
                    scale: true,
                    min_scale: 1,
                    max_scale: u32::MAX,
                },
                color: ColorLimits {
                    operations: 16,
                    srgb: true,
                    plane_matrix: true,
                    output_matrix: true,
                    lut_entries: 256,
                    yuv_encodings: [true; 3],
                    yuv_ranges: [true; 2],
                },
                layers: crate::scene::MAX_PLANES,
                roles: [1, 22, 1],
            },
            formats,
        )?,
        GFP_KERNEL,
    )?))
}

#[kunit_tests(rust_castkms_validation)]
mod cases {
    use super::*;

    #[test]
    fn renderer_contract_does_not_inherit_host_dimensions() -> Result {
        let scene = Scene::blank(None);
        let view = SceneView::Enabled {
            scene: &scene,
            output: [16384; 2],
        };
        renderer([16384; 2])?.check(view)?;
        assert_eq!(Contract::Host.check(view), Err(EOPNOTSUPP));
        Ok(())
    }

    #[test]
    fn disabled_is_not_an_enabled_empty_scene() -> Result {
        let contract = renderer([4; 2])?;
        contract.check(SceneView::Disabled)?;
        let scene = Scene::blank(None);
        assert_eq!(
            contract.check(SceneView::Enabled {
                scene: &scene,
                output: [8; 2],
            }),
            Err(EOPNOTSUPP)
        );
        assert_eq!(
            contract.check(SceneView::Enabled {
                scene: &scene,
                output: [0; 2],
            }),
            Err(EOPNOTSUPP)
        );
        Ok(())
    }


}
