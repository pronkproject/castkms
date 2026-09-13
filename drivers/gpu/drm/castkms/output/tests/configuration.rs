// SPDX-License-Identifier: GPL-2.0-only

//! Configuration metadata follows accepted publication, independently of scene representation.

use super::*;

#[kunit_tests(rust_castkms_output_configuration)]
mod cases {
    use super::*;
    use core::sync::atomic::{
        AtomicUsize,
        Ordering, //
    };

    struct Metadata(Arc<AtomicUsize>);

    impl Drop for Metadata {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn replacing_and_closing_release_owned_metadata() -> Result {
        let drops = Arc::new(AtomicUsize::new(0), GFP_KERNEL)?;
        let output = Arc::pin_init(Output::<u32, Metadata>::new(), GFP_KERNEL)?;
        output.publish_with_configuration(
            Source::new(1)?,
            SceneUpdate::Replace(Some(17)),
            Metadata(drops.clone()),
        );
        output.publish_with_configuration(
            Source::new(1)?,
            SceneUpdate::Retain,
            Metadata(drops.clone()),
        );
        if drops.load(Ordering::Relaxed) != 1 {
            return Err(EINVAL);
        }
        output.close();
        if drops.load(Ordering::Relaxed) != 2 {
            return Err(EINVAL);
        }
        let late = Source::new(1)?;
        output.publish_with_configuration(
            late.clone(),
            SceneUpdate::Replace(None),
            Metadata(drops.clone()),
        );
        if drops.load(Ordering::Relaxed) != 3 || !matches!(late.claim(), Err(EBUSY)) {
            return Err(EINVAL);
        }
        Ok(())
    }

    #[test]
    fn retained_pixels_receive_the_new_configuration() -> Result {
        let output = Arc::pin_init(Output::<u32, u32>::new(), GFP_KERNEL)?;
        let first = Source::new(1)?;
        let second = Source::new(1)?;
        output.publish_with_configuration(first.clone(), SceneUpdate::Replace(Some(17)), 3);
        output.publish_with_configuration(second.clone(), SceneUpdate::Retain, 5);
        let accepted = output.with_accepted(|accepted| {
            accepted.map(|accepted| (accepted.scene.copied(), *accepted.configuration))
        });
        if accepted != Some((Some(17), 5)) || !matches!(first.claim(), Err(EBUSY)) {
            return Err(EINVAL);
        }
        second.claim()?.release_cpu();
        Ok(())
    }

    #[test]
    fn blank_configuration_is_distinct_from_no_publication() -> Result {
        let output = Arc::pin_init(Output::<u32, u32>::new(), GFP_KERNEL)?;
        if output.with_accepted(|accepted| accepted.is_some()) {
            return Err(EINVAL);
        }
        output.publish_with_configuration(Source::new(1)?, SceneUpdate::Replace(None), 7);
        let accepted = output.with_accepted(|accepted| {
            accepted.map(|accepted| (accepted.scene.copied(), *accepted.configuration))
        });
        if accepted != Some((None, 7)) {
            return Err(EINVAL);
        }
        output.close();
        if output.with_accepted(|accepted| accepted.is_some()) {
            return Err(EINVAL);
        }
        Ok(())
    }

    #[test]
    fn an_active_read_keeps_the_configuration_of_its_pixels() -> Result {
        let output = Arc::pin_init(Output::<u32, u32>::new(), GFP_KERNEL)?;
        let first = Source::new(1)?;
        let second = Source::new(1)?;
        output.publish_with_configuration(first.clone(), SceneUpdate::Replace(Some(17)), 3);
        let observed = output.with_prepared_cpu_scene(
            |_| Ok(()),
            |scene, configuration, ()| {
                output.publish_with_configuration(second, SceneUpdate::Replace(Some(23)), 5);
                (*scene, *configuration)
            },
        )?;
        if observed != Some((17, 3)) || first.prepared()?.is_none() {
            return Err(EINVAL);
        }
        let current = output.with_prepared_cpu_scene(
            |_| Ok(()),
            |scene, configuration, ()| (*scene, *configuration),
        )?;
        if current != Some((23, 5)) {
            return Err(EINVAL);
        }
        Ok(())
    }

    #[test]
    fn replaced_metadata_cannot_be_paired_with_prepared_pixels() -> Result {
        let output = Arc::pin_init(Output::<u32, u32>::new(), GFP_KERNEL)?;
        let first = Source::new(1)?;
        let second = Source::new(1)?;
        output.publish_with_configuration(first.clone(), SceneUpdate::Replace(Some(17)), 3);
        let mut read = false;
        let result = output.with_prepared_cpu_scene(
            |_| {
                output.publish_with_configuration(second, SceneUpdate::Replace(Some(23)), 5);
                Ok(())
            },
            |_, _, ()| read = true,
        );
        if result != Err(EBUSY) || read || first.prepared()?.is_none() {
            return Err(EINVAL);
        }
        Ok(())
    }
}
