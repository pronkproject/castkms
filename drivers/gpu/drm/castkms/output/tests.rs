// SPDX-License-Identifier: GPL-2.0-only

use super::*;

#[kunit_tests(rust_castkms_output)]
mod cases {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use kernel::sync::Arc;

    struct Resource(Arc<AtomicUsize>);

    impl Drop for Resource {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn replacement_releases_previous_resource() -> Result {
        let drops = Arc::new(AtomicUsize::new(0), GFP_KERNEL)?;
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        output.publish(Some(Resource(drops.clone())));
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        output.publish(Some(Resource(drops.clone())));
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        output.close();
        assert_eq!(drops.load(Ordering::Relaxed), 2);
        Ok(())
    }

    #[test]
    fn blank_releases_source_without_closing_output() -> Result {
        let drops = Arc::new(AtomicUsize::new(0), GFP_KERNEL)?;
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        output.publish(Some(Resource(drops.clone())));
        output.publish(None);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        output.publish(Some(Resource(drops.clone())));
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        output.close();
        assert_eq!(drops.load(Ordering::Relaxed), 2);
        Ok(())
    }

    #[test]
    fn close_rejects_late_publication() -> Result {
        let drops = Arc::new(AtomicUsize::new(0), GFP_KERNEL)?;
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        output.publish(Some(Resource(drops.clone())));
        output.close();
        output.publish(Some(Resource(drops.clone())));
        output.publish(None);
        output.close();
        assert_eq!(drops.load(Ordering::Relaxed), 2);
        assert!(matches!(*output.state.lock(), Publication::Closed));
        Ok(())
    }

    #[test]
    fn discarded_candidate_does_not_replace_current() -> Result {
        let drops = Arc::new(AtomicUsize::new(0), GFP_KERNEL)?;
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        output.publish(Some(Resource(drops.clone())));
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
        output.publish(Some(Resource(drops.clone())));
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
        output.publish(Some(Reenter(output.clone())));
        output.publish(None);
        assert!(matches!(*output.state.lock(), Publication::Closed));
        Ok(())
    }
}
