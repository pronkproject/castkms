// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::output::{
    Output,
    SceneUpdate, //
};
use core::sync::atomic::{
    AtomicUsize,
    Ordering, //
};
use kernel::{
    drm::preparation::Source,
    sync::Arc, //
};

struct Resource(Arc<AtomicUsize>);

impl Drop for Resource {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[kunit_tests(rust_castkms_output_identity)]
mod cases {
    use super::*;

    #[test]
    fn retained_identity_does_not_retain_publication_resources() -> Result {
        let destroyed = Arc::new(AtomicUsize::new(0), GFP_KERNEL)?;
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        let source = Source::new(1)?;
        output.publish(
            source.clone(),
            SceneUpdate::Replace(Some(Resource(destroyed.clone()))),
        );
        let identity = output.identity().clone();
        drop(output);
        check(destroyed.load(Ordering::Relaxed) == 1)?;
        check(matches!(source.claim(), Err(EBUSY)))?;
        for _ in 0..64 {
            let replacement = Arc::pin_init(Output::<u32>::new(), GFP_KERNEL)?;
            check(&identity != replacement.identity())?;
        }
        Ok(())
    }

    #[test]
    fn identity_is_stable_across_scenes_blanking_and_shutdown() -> Result {
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        let identity = output.identity().clone();
        output.publish(Source::new(1)?, SceneUpdate::Replace(Some(17)));
        check(&identity == output.identity())?;
        output.publish(Source::new(1)?, SceneUpdate::Replace(Some(23)));
        check(&identity == output.identity())?;
        output.publish(Source::new(1)?, SceneUpdate::Replace(None));
        check(&identity == output.identity())?;
        output.close();
        check(&identity == output.identity())?;
        Ok(())
    }
}
