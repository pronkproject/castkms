// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::drm::preparation::{
    OutputGeneration,
    RetirementSet,
    Source, //
};

fn release_file(file: ARef<File>) {
    // SAFETY: KUnit runs in a kernel thread. Transfer one owned file reference and finish its
    // release synchronously so the test observes the cancellation boundary before continuing.
    unsafe { bindings::__fput_sync(ARef::into_raw(file).cast().as_ptr()) };
}

#[kunit_tests(rust_drm_preparation_file)]
mod cases {
    use super::*;

    #[test]
    fn file_cancellation_is_independent_of_kernel_ticket_references() -> Result {
        let source = Source::new(1)?;
        let set = RetirementSet::new(&[source.clone()])?;
        let ticket = Ticket::new(&[OutputGeneration::new(1, &source)?])?;
        drop(set);
        let file = ticket.create_file()?;
        let retained = Ticket::from_file(&file)?;
        let duplicate = file.clone();
        release_file(file);
        retained.wait_ready()?;
        release_file(duplicate);
        assert_eq!(retained.wait_ready(), Err(ECANCELED));
        assert_eq!(ticket.wait_ready(), Err(ECANCELED));
        source.claim()?.release_cpu();
        Ok(())
    }

    #[test]
    fn file_close_does_not_release_an_active_attempt() -> Result {
        let source = Source::new(1)?;
        let set = RetirementSet::new(&[source.clone()])?;
        let ticket = Ticket::new(&[OutputGeneration::new(1, &source)?])?;
        drop(set);
        let file = ticket.create_file()?;
        let attempt = ticket.reserve()?;
        release_file(file);
        assert_eq!(ticket.wait_ready(), Err(ECANCELED));
        assert!(matches!(source.claim(), Err(EBUSY)));
        drop(attempt);
        source.claim()?.release_cpu();
        Ok(())
    }
}
