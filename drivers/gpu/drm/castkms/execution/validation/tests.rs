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
    fn activation_replaces_both_contracts_only_after_publication() -> Result {
        let mut validation = Validation::new(Contract::Host);
        assert!(matches!(validation.prepare_activation(2, |contract| contract.check(SceneView::Disabled)), Err(ESTALE)));
        let epoch = validation.epoch();
        validation.prepare(epoch, 2, renderer([16384; 2])?, SceneView::Disabled)?.commit();
        let gated = validation.epoch();
        drop(validation.prepare_activation(2, |contract| contract.check(SceneView::Disabled))?);
        assert_eq!(validation.epoch(), gated);
        let scene = Scene::blank(None);
        let large = SceneView::Enabled { scene: &scene, output: [16384; 2] };
        assert_eq!(validation.check(large), Err(EOPNOTSUPP));
        let retired = validation.prepare_activation(2, |contract| contract.check(SceneView::Disabled))?.commit();
        assert_ne!(validation.epoch(), gated);
        validation.check(large)?;
        assert!(validation.cancel(2).is_none());
        assert!(matches!(validation.prepare_activation(2, |contract| contract.check(SceneView::Disabled)), Err(ESTALE)));
        drop(retired);
        Ok(())
    }

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
    fn installed_gate_checks_both_contracts_until_cancelled() -> Result {
        let mut state = Validation::new(renderer([4, 8])?);
        let epoch = state.epoch();
        state
            .prepare(epoch, 7, renderer([8, 4])?, SceneView::Disabled)?
            .commit();
        assert_ne!(state.epoch(), epoch);
        let scene = Scene::blank(None);
        state.check(SceneView::Enabled {
            scene: &scene,
            output: [4; 2],
        })?;
        for output in [[4, 8], [8, 4]] {
            assert_eq!(
                state.check(SceneView::Enabled {
                    scene: &scene,
                    output
                }),
                Err(EOPNOTSUPP)
            );
        }
        let installed = state.epoch();
        assert!(state.cancel(6).is_none());
        assert_eq!(state.epoch(), installed);
        drop(state.cancel(7).ok_or(EINVAL)?);
        assert_ne!(state.epoch(), installed);
        state.check(SceneView::Enabled {
            scene: &scene,
            output: [4, 8],
        })?;
        assert_eq!(
            state.check(SceneView::Enabled {
                scene: &scene,
                output: [8, 4],
            }),
            Err(EOPNOTSUPP)
        );
        let cancelled = state.epoch();
        assert!(state.cancel(7).is_none());
        assert_eq!(state.epoch(), cancelled);
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
    fn an_old_epoch_cannot_install_after_gate_cancellation() -> Result {
        let mut state = Validation::new(Contract::Host);
        let old = state.epoch();
        state
            .prepare(old, 1, Contract::Host, SceneView::Disabled)?
            .commit();
        let installed = state.epoch();
        assert!(matches!(
            state.prepare(installed, 2, Contract::Host, SceneView::Disabled),
            Err(EBUSY)
        ));
        drop(state.cancel(1));
        assert!(matches!(
            state.prepare(old, 2, Contract::Host, SceneView::Disabled),
            Err(ESTALE)
        ));
        let current = state.epoch();
        state
            .prepare(current, 2, Contract::Host, SceneView::Disabled)?
            .commit();
        Ok(())
    }

    #[test]
    fn gate_installation_reserves_the_cancellation_epoch() -> Result {
        let mut state = Validation::new(Contract::Host);
        state.epoch = Epoch(u64::MAX - 2);
        let epoch = state.epoch();
        state
            .prepare(epoch, 1, Contract::Host, SceneView::Disabled)?
            .commit();
        assert_eq!(state.epoch(), Epoch(u64::MAX - 1));
        drop(state.cancel(1).ok_or(EINVAL)?);
        assert_eq!(state.epoch(), Epoch(u64::MAX));
        state.check(SceneView::Disabled)?;
        let epoch = state.epoch();
        assert!(matches!(
            state.prepare(epoch, 2, Contract::Host, SceneView::Disabled),
            Err(EOVERFLOW)
        ));
        assert!(state.gate.is_none());
        state.epoch = Epoch(u64::MAX - 1);
        let epoch = state.epoch();
        assert!(matches!(
            state.prepare(epoch, 2, Contract::Host, SceneView::Disabled),
            Err(EOVERFLOW)
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
