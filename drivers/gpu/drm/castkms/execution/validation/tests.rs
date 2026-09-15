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

    #[test]
    fn abandoned_installation_preserves_epoch_and_acceptance() -> Result {
        let mut state = Validation::new(Contract::Host);
        let epoch = state.epoch();
        let installation = state.prepare(epoch, 1, renderer([4; 2])?, SceneView::Disabled)?;
        drop(installation);
        assert_eq!(state.epoch(), epoch);
        assert!(state.gate.is_none());
        let scene = Scene::blank(None);
        state.check(SceneView::Enabled {
            scene: &scene,
            output: [8; 2],
        })?;
        Ok(())
    }

    #[test]
    fn failed_validation_does_not_install_a_gate() -> Result {
        let mut state = Validation::new(Contract::Host);
        let epoch = state.epoch();
        let scene = Scene::blank(None);
        assert!(matches!(
            state.prepare(
                epoch,
                1,
                renderer([4; 2])?,
                SceneView::Enabled {
                    scene: &scene,
                    output: [8; 2]
                }
            ),
            Err(EOPNOTSUPP)
        ));
        assert_eq!(state.epoch(), epoch);
        assert!(state.gate.is_none());
        assert!(matches!(
            state.prepare(epoch, 0, Contract::Host, SceneView::Disabled),
            Err(EINVAL)
        ));
        Ok(())
    }

    #[test]
    fn compatible_updates_do_not_advance_the_validation_epoch() -> Result {
        let mut state = Validation::new(Contract::Host);
        let epoch = state.epoch();
        state
            .prepare(epoch, 1, renderer([8; 2])?, SceneView::Disabled)?
            .commit();
        let epoch = state.epoch();
        for size in [4, 8, 2, 8] {
            let scene = Scene::blank(None);
            state.check(SceneView::Enabled {
                scene: &scene,
                output: [size; 2],
            })?;
            assert_eq!(state.epoch(), epoch);
        }
        Ok(())
    }

    #[test]
    fn one_outputs_gate_does_not_restrict_another() -> Result {
        let mut first = Validation::new(Contract::Host);
        let second = Validation::new(Contract::Host);
        let epoch = first.epoch();
        first
            .prepare(epoch, 1, renderer([4; 2])?, SceneView::Disabled)?
            .commit();
        let scene = Scene::blank(None);
        let view = SceneView::Enabled {
            scene: &scene,
            output: [8; 2],
        };
        assert_eq!(first.check(view), Err(EOPNOTSUPP));
        second.check(view)?;
        assert_eq!(second.epoch(), epoch);
        Ok(())
    }
}
