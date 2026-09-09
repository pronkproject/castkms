// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Native transaction preparation through the ordinary Rust test driver's commit path.

use super::*;
use crate::{
    drm::preparation::{
        RetirementSet,
        Source,
        Ticket, //
    },
    error::to_result, //
};

// The fixture's sources have no pixel access. Its single-task update callback owns the entire
// simulated scope through installation; no source mapping or external authority is delegated.
fn attach(state: &atomic::AtomicStateComposer<TestDriver>, ticket: &Ticket) -> Result {
    // SAFETY: Ticket transparently represents the initialized native ticket. The composer
    // borrows an unpublished, exclusively accessed transaction. The fixture establishes the
    // complete synthetic source scope above, independently of any framebuffer metadata.
    unsafe {
        to_result(bindings::drm_atomic_commit_prepare(
            state.as_raw(),
            core::ptr::from_ref(ticket).cast_mut().cast(),
        ))
    }
}

#[kunit_tests(rust_drm_transaction_preparation)]
mod cases {
    use super::*;

    #[test]
    fn failed_framebuffer_preparation_abandons_ticket_reservation() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-ticket-retry", None)?;
        let dev = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let fb = framebuffer(dev.device())?;
        let mode = mode()?;
        let scanout = atomic::CrtcScanout {
            mode: &mode,
            framebuffer: &fb,
            connectors: &[dev.connector()?],
            position: (0, 0),
        };
        let source = Source::new(1)?;
        let set = RetirementSet::new(&[source.clone()])?;
        let ticket = Ticket::new(&set)?;
        drop(set);
        counts
            .fail_framebuffer_preparation
            .store(1, Ordering::Relaxed);
        assert_eq!(
            dev.update(|mut state| {
                state
                    .as_mut()
                    .set_crtc_config(dev.crtc()?, Some(&scanout))?;
                attach(&state, &ticket)
            }),
            Err(ENOMEM)
        );
        assert_eq!(counts.plane_updates.load(Ordering::Relaxed), 0);
        let retry = ticket.reserve()?;
        drop(retry);
        counts
            .fail_framebuffer_preparation
            .store(0, Ordering::Relaxed);
        dev.update(|mut state| {
            state
                .as_mut()
                .set_crtc_config(dev.crtc()?, Some(&scanout))?;
            attach(&state, &ticket)
        })?;
        assert_eq!(counts.plane_updates.load(Ordering::Relaxed), 1);
        assert!(matches!(ticket.reserve(), Err(EALREADY)));
        source.claim()?.release_cpu();
        drop(fb);
        drop(dev);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.plane_states.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn cancellation_rejects_update_before_driver_programming() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-ticket-cancel", None)?;
        let dev = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let fb = framebuffer(dev.device())?;
        let mode = mode()?;
        let scanout = atomic::CrtcScanout {
            mode: &mode,
            framebuffer: &fb,
            connectors: &[dev.connector()?],
            position: (0, 0),
        };
        let source = Source::new(1)?;
        let set = RetirementSet::new(&[source.clone()])?;
        let ticket = Ticket::new(&set)?;
        drop(set);
        assert_eq!(
            dev.update(|mut state| {
                state
                    .as_mut()
                    .set_crtc_config(dev.crtc()?, Some(&scanout))?;
                attach(&state, &ticket)?;
                ticket.cancel();
                Ok(())
            }),
            Err(ECANCELED)
        );
        assert_eq!(counts.plane_updates.load(Ordering::Relaxed), 0);
        assert_eq!(counts.enables.load(Ordering::Relaxed), 0);
        source.claim()?.release_cpu();
        dev.update(|state| state.set_crtc_config(dev.crtc()?, Some(&scanout)))?;
        assert_eq!(counts.plane_updates.load(Ordering::Relaxed), 1);
        drop(fb);
        drop(dev);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.plane_states.load(Ordering::Relaxed), 0);
        Ok(())
    }
}
