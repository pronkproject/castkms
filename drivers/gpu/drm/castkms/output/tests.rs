// SPDX-License-Identifier: GPL-2.0-only

use super::*;

#[kunit_tests(rust_castkms_output)]
mod cases {
    use super::*;
    use crate::{
        provenance::{PreviousOwner, Provenance, Selection},
        scene::ContentSerial,
    };
    use core::sync::atomic::{AtomicUsize, Ordering};
    use kernel::sync::Arc;

    struct Resource(Arc<AtomicUsize>);

    #[test]
    fn retained_scene_moves_to_the_new_generation() -> Result {
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        let first = Source::new(2)?;
        let second = Source::new(2)?;
        output.publish(first.clone(), SceneUpdate::Replace(Some(17)));
        let hold = first.hold_admission()?;
        assert!(hold.prepared()?.is_some());
        output.publish(second.clone(), SceneUpdate::Retain);
        drop(hold);
        assert!(matches!(first.claim(), Err(EBUSY)));
        second.claim()?.release_cpu();
        assert!(
            output.inspect_accepted(|current| current.is_some_and(|(source, scene)| {
                core::ptr::eq(source, &*second) && scene == Some(&17)
            }))
        );
        Ok(())
    }

    #[test]
    fn blank_publication_keeps_its_generation() -> Result {
        let output = Arc::pin_init(Output::<u32>::new(), GFP_KERNEL)?;
        let source = Source::new(2)?;
        output.publish(source.clone(), SceneUpdate::Replace(None));
        assert!(
            output.inspect_accepted(|current| current.is_some_and(|(published, scene)| {
                core::ptr::eq(published, &*source) && scene.is_none()
            }))
        );
        output.close();
        assert!(matches!(source.claim(), Err(EBUSY)));
        Ok(())
    }

    #[test]
    fn closing_rejects_a_late_generation() -> Result {
        let output = Arc::pin_init(Output::<u32>::new(), GFP_KERNEL)?;
        let late = Source::new(2)?;
        output.close();
        output.publish(late.clone(), SceneUpdate::Retain);
        assert!(output.inspect_accepted(|current| current.is_none()));
        assert!(matches!(late.claim(), Err(EBUSY)));
        Ok(())
    }

    #[test]
    fn discarded_attribution_does_not_change_publication() -> Result {
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        let first_content = ContentSerial::for_update(None, true)?;
        let first_owner = Provenance::for_update(
            None,
            PreviousOwner::DifferentFramebuffer,
            Some(&1),
            Selection::DifferentFramebuffer,
        )
        .copied();
        output.publish(
            Source::new(2)?,
            SceneUpdate::Replace(Some((first_owner, first_content))),
        );
        let candidate_owner = Provenance::for_update(
            None,
            PreviousOwner::DifferentFramebuffer,
            Some(&2),
            Selection::DifferentFramebuffer,
        )
        .copied();
        let candidate_content = ContentSerial::for_update(first_content, true)?;
        // Preparing a candidate is not publication. A failed or test-only transaction
        // discards its candidate without calling the accepted-update publisher.
        {
            let state = output.state.lock();
            match &*state {
                Publication::Open(Some(Generation {
                    scene: Some((owner, content)),
                    ..
                })) => {
                    assert_eq!(*owner, first_owner);
                    assert_eq!(*content, first_content);
                }
                _ => return Err(EINVAL),
            }
        }
        output.publish(
            Source::new(2)?,
            SceneUpdate::Replace(Some((candidate_owner, candidate_content))),
        );
        let state = output.state.lock();
        match &*state {
            Publication::Open(Some(Generation {
                scene: Some((owner, content)),
                ..
            })) => {
                assert_eq!(*owner, Some(2));
                assert_eq!(*content, candidate_content);
            }
            _ => return Err(EINVAL),
        }
        Ok(())
    }

    #[test]
    fn same_image_updates_content_without_adopting_current_master() -> Result {
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        let first_content = ContentSerial::for_update(None, true)?;
        let next_content = ContentSerial::for_update(first_content, true)?;
        assert!(first_content != next_content);
        let owner = Provenance::for_update(
            None,
            PreviousOwner::SameFramebuffer(Some(&1)),
            Some(&2),
            Selection::RetainedFramebuffer,
        )
        .copied();
        output.publish(
            Source::new(2)?,
            SceneUpdate::Replace(Some((owner, next_content))),
        );
        {
            let state = output.state.lock();
            match &*state {
                Publication::Open(Some(Generation {
                    scene: Some((owner, content)),
                    ..
                })) => {
                    assert_eq!(*owner, Some(1));
                    assert_eq!(*content, next_content);
                }
                _ => return Err(EINVAL),
            }
        }
        output.publish(Source::new(2)?, SceneUpdate::Replace(None));
        assert!(matches!(
            *output.state.lock(),
            Publication::Open(Some(Generation { scene: None, .. }))
        ));
        output.close();
        output.publish(
            Source::new(2)?,
            SceneUpdate::Replace(Some((Some(2), next_content))),
        );
        assert!(matches!(*output.state.lock(), Publication::Closed));
        Ok(())
    }

    impl Drop for Resource {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn replacement_releases_previous_resource() -> Result {
        let drops = Arc::new(AtomicUsize::new(0), GFP_KERNEL)?;
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        output.publish(
            Source::new(2)?,
            SceneUpdate::Replace(Some(Resource(drops.clone()))),
        );
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        output.publish(
            Source::new(2)?,
            SceneUpdate::Replace(Some(Resource(drops.clone()))),
        );
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        output.close();
        assert_eq!(drops.load(Ordering::Relaxed), 2);
        Ok(())
    }

    #[test]
    fn blank_releases_source_without_closing_output() -> Result {
        let drops = Arc::new(AtomicUsize::new(0), GFP_KERNEL)?;
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        output.publish(
            Source::new(2)?,
            SceneUpdate::Replace(Some(Resource(drops.clone()))),
        );
        output.publish(Source::new(2)?, SceneUpdate::Replace(None));
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        output.publish(
            Source::new(2)?,
            SceneUpdate::Replace(Some(Resource(drops.clone()))),
        );
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        output.close();
        assert_eq!(drops.load(Ordering::Relaxed), 2);
        Ok(())
    }

    #[test]
    fn close_rejects_late_publication() -> Result {
        let drops = Arc::new(AtomicUsize::new(0), GFP_KERNEL)?;
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        output.publish(
            Source::new(2)?,
            SceneUpdate::Replace(Some(Resource(drops.clone()))),
        );
        output.close();
        output.publish(
            Source::new(2)?,
            SceneUpdate::Replace(Some(Resource(drops.clone()))),
        );
        output.publish(Source::new(2)?, SceneUpdate::Replace(None));
        output.close();
        assert_eq!(drops.load(Ordering::Relaxed), 2);
        assert!(matches!(*output.state.lock(), Publication::Closed));
        Ok(())
    }

    #[test]
    fn discarded_candidate_does_not_replace_current() -> Result {
        let drops = Arc::new(AtomicUsize::new(0), GFP_KERNEL)?;
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        output.publish(
            Source::new(2)?,
            SceneUpdate::Replace(Some(Resource(drops.clone()))),
        );
        drop(Resource(drops.clone()));
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        assert!(matches!(*output.state.lock(), Publication::Open(Some(_))));
        output.close();
        assert_eq!(drops.load(Ordering::Relaxed), 2);
        Ok(())
    }

    #[test]
    fn output_destruction_releases_current() -> Result {
        let drops = Arc::new(AtomicUsize::new(0), GFP_KERNEL)?;
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        output.publish(
            Source::new(2)?,
            SceneUpdate::Replace(Some(Resource(drops.clone()))),
        );
        drop(output);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        Ok(())
    }

    struct Reenter(Arc<Output<Reenter>>);

    impl Drop for Reenter {
        fn drop(&mut self) {
            self.0.close();
        }
    }

    #[test]
    fn destruction_runs_outside_publication_lock() -> Result {
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        output.publish(
            Source::new(2)?,
            SceneUpdate::Replace(Some(Reenter(output.clone()))),
        );
        output.publish(Source::new(2)?, SceneUpdate::Replace(None));
        assert!(matches!(*output.state.lock(), Publication::Closed));
        Ok(())
    }
}
