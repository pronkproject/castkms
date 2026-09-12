// SPDX-License-Identifier: GPL-2.0-only

use super::*;

#[kunit_tests(rust_castkms_cpu_reads)]
mod cases {
    use super::*;
    use kernel::sync::Arc;

    #[test]
    fn preparation_waits_until_the_cpu_callback_returns() -> Result {
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        let source = Source::new(2)?;
        output.publish(source.clone(), SceneUpdate::Replace(Some(17)));
        let mut hold = None;
        let (value, pending) = output
            .with_cpu_scene(|scene| -> Result<_> {
                hold = Some(source.hold_admission()?);
                let pending = hold.as_ref().ok_or(EINVAL)?.prepared()?.is_none();
                Ok((*scene, pending))
            })?
            .ok_or(EINVAL)??;
        assert_eq!(value, 17);
        assert!(pending);
        assert!(hold.ok_or(EINVAL)?.prepared()?.is_some());
        Ok(())
    }

    #[test]
    fn callback_failure_releases_the_cpu_claim() -> Result {
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        let source = Source::new(1)?;
        output.publish(source.clone(), SceneUpdate::Replace(Some(17)));
        assert_eq!(
            output.with_cpu_scene(|_| Err::<(), _>(EIO))?,
            Some(Err(EIO))
        );
        source.claim()?.release_cpu();
        assert!(source.hold_admission()?.prepared()?.is_some());
        Ok(())
    }

    #[test]
    fn admission_hold_prevents_the_callback() -> Result {
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        let source = Source::new(1)?;
        output.publish(source.clone(), SceneUpdate::Replace(Some(17)));
        let hold = source.hold_admission()?;
        let mut called = false;
        assert_eq!(output.with_cpu_scene(|_| called = true), Err(EBUSY));
        assert!(!called);
        drop(hold);
        assert_eq!(output.with_cpu_scene(|scene| *scene)?, Some(17));
        Ok(())
    }

    #[test]
    fn blank_and_closed_outputs_do_not_call_the_reader() -> Result {
        let output = Arc::pin_init(Output::<u32>::new(), GFP_KERNEL)?;
        let mut called = false;
        assert_eq!(output.with_cpu_scene(|_| called = true)?, None);
        output.publish(Source::new(1)?, SceneUpdate::Replace(None));
        assert_eq!(output.with_cpu_scene(|_| called = true)?, None);
        output.close();
        assert_eq!(output.with_cpu_scene(|_| called = true)?, None);
        assert!(!called);
        Ok(())
    }

    #[test]
    fn replacement_keeps_the_active_reader_on_its_generation() -> Result {
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        let old = Source::new(1)?;
        let replacement = Source::new(1)?;
        output.publish(old.clone(), SceneUpdate::Replace(Some(17)));
        let observed = output
            .with_cpu_scene(|scene| -> Result<_> {
                output.publish(replacement.clone(), SceneUpdate::Replace(Some(23)));
                Ok((
                    *scene,
                    old.prepared()?.is_none(),
                    output.with_cpu_scene(|current| *current)?,
                ))
            })?
            .ok_or(EINVAL)??;
        assert_eq!(observed, (17, true, Some(23)));
        assert!(old.prepared()?.is_some());
        assert!(matches!(old.claim(), Err(EBUSY)));
        replacement.claim()?.release_cpu();
        Ok(())
    }

    #[test]
    fn resources_are_prepared_without_a_source_claim() -> Result {
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        let source = Source::new(1)?;
        output.publish(source.clone(), SceneUpdate::Replace(Some(17)));
        let value = output.with_prepared_cpu_scene(
            |_| {
                let hold = source.hold_admission()?;
                if hold.prepared()?.is_none() {
                    return Err(EBUSY);
                }
                Ok(23)
            },
            |scene, resource| *scene + resource,
        )?;
        assert_eq!(value, Some(40));
        Ok(())
    }

    #[test]
    fn source_read_ends_before_prepared_resources_are_destroyed() -> Result {
        use core::cell::Cell;

        struct Resource<'a> {
            source: &'a Source,
            released: &'a Cell<bool>,
        }

        impl Drop for Resource<'_> {
            fn drop(&mut self) {
                self.released
                    .set(matches!(self.source.prepared(), Ok(Some(_))));
            }
        }

        for fail in [false, true] {
            let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
            let source = Source::new(1)?;
            output.publish(source.clone(), SceneUpdate::Replace(Some(17)));
            let released = Cell::new(false);
            let result = output.with_prepared_cpu_scene(
                |_| {
                    Ok(Resource {
                        source: &source,
                        released: &released,
                    })
                },
                |_, _resource| {
                    output.close();
                    assert!(source.prepared()?.is_none());
                    if fail {
                        Err(EIO)
                    } else {
                        Ok(())
                    }
                },
            )?;
            assert_eq!(result, Some(if fail { Err(EIO) } else { Ok(()) }));
            assert!(released.get());
        }
        Ok(())
    }

    #[test]
    fn replacement_during_resource_preparation_prevents_reading() -> Result {
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        let old = Source::new(1)?;
        output.publish(old.clone(), SceneUpdate::Replace(Some(17)));
        let replacement = Source::new(1)?;
        let mut called = false;
        let result = output.with_prepared_cpu_scene(
            |_| {
                output.publish(replacement, SceneUpdate::Replace(Some(23)));
                Ok(())
            },
            |_, ()| called = true,
        );
        assert_eq!(result, Err(EBUSY));
        assert!(!called);
        assert!(old.prepared()?.is_some());
        assert_eq!(output.with_cpu_scene(|scene| *scene)?, Some(23));
        Ok(())
    }

    #[test]
    fn failed_resource_preparation_does_not_consume_admission() -> Result {
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        let source = Source::new(1)?;
        output.publish(source.clone(), SceneUpdate::Replace(Some(17)));
        let mut called = false;
        assert_eq!(
            output.with_prepared_cpu_scene(|_| Err::<(), _>(ENOMEM), |_, ()| called = true),
            Err(ENOMEM)
        );
        assert!(!called);
        source.claim()?.release_cpu();
        Ok(())
    }

    #[test]
    fn callback_runs_outside_the_publication_lock() -> Result {
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        let source = Source::new(1)?;
        output.publish(source.clone(), SceneUpdate::Replace(Some(17)));
        let result = output.with_cpu_scene(|scene| {
            output.close();
            *scene
        })?;
        assert_eq!(result, Some(17));
        assert!(source.prepared()?.is_some());
        Ok(())
    }
}
