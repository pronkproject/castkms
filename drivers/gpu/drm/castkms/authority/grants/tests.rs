// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use core::sync::atomic::{
    AtomicBool,
    AtomicU32,
    Ordering, //
};
use kernel::drm::capture::{
    Authority,
    Policy, //
};
use kernel::workqueue::{
    self,
    impl_has_work,
    new_work,
    Work,
    WorkItem, //
};

#[derive(Default)]
struct Counts {
    revokes: AtomicU32,
    releases: AtomicU32,
    registration_denied: AtomicBool,
}

struct Probe {
    registry: Arc<Registry>,
    other: ARef<Revocation>,
}

struct TestPolicy {
    counts: Arc<Counts>,
    probe: Option<Probe>,
}

// SAFETY: The callbacks and destructor belong to CastKMS's local module.
#[vtable]
unsafe impl Policy for TestPolicy {
    fn revoke(&self) {
        if let Some(probe) = &self.probe {
            let result = probe.registry.register(&probe.other);
            self.counts
                .registration_denied
                .store(matches!(result, Err(ENODEV)), Ordering::Relaxed);
        }
        self.counts.revokes.fetch_add(1, Ordering::Relaxed);
    }
}

impl Drop for TestPolicy {
    fn drop(&mut self) {
        self.counts.releases.fetch_add(1, Ordering::Relaxed);
    }
}

fn authority(counts: &Arc<Counts>, probe: Option<Probe>) -> Result<ARef<Authority<TestPolicy>>> {
    Authority::new(Arc::new(
        TestPolicy {
            counts: counts.clone(),
            probe,
        },
        GFP_KERNEL,
    )?)
}

#[track_caller]
fn check(condition: bool) -> Result {
    if condition {
        Ok(())
    } else {
        let location = core::panic::Location::caller();
        pr_err!(
            "Grant registry check failed at {}:{}\n",
            location.file(),
            location.line()
        );
        Err(EINVAL)
    }
}

#[pin_data]
struct Gate {
    #[pin]
    entered: Completion,
    #[pin]
    proceed: Completion,
}

struct GatedPolicy {
    gate: Arc<Gate>,
    counts: Arc<Counts>,
}

// SAFETY: The callbacks and destructor belong to CastKMS's local module.
#[vtable]
unsafe impl Policy for GatedPolicy {
    fn revoke(&self) {
        self.gate.entered.complete_all();
        self.gate.proceed.wait_for_completion();
        self.counts.revokes.fetch_add(1, Ordering::Relaxed);
    }
}

/// Opens the gate before any partial grant or registration is destroyed.
struct GatedGrant {
    authority: ARef<Authority<GatedPolicy>>,
    registration: Option<Registration>,
    gate: Arc<Gate>,
}

impl Drop for GatedGrant {
    fn drop(&mut self) {
        self.gate.proceed.complete_all();
    }
}

#[pin_data]
struct Closer {
    #[pin]
    work: Work<Self>,
    #[pin]
    started: Completion,
    registry: Arc<Registry>,
    gate: Arc<Gate>,
    counts: Arc<Counts>,
    revokes_seen: AtomicU32,
}

impl_has_work! {
    impl HasWork<Self> for Closer { self.work }
}

impl WorkItem for Closer {
    type Pointer = Arc<Self>;

    fn run(closer: Arc<Self>) {
        closer.started.complete_all();
        closer.registry.close();
        closer.revokes_seen.store(
            closer.counts.revokes.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
    }
}

/// Every owner releases the callback gate before flushing, including failed-test exits.
struct Closing(Arc<Closer>);

impl Closing {
    fn start(registry: &Arc<Registry>, gate: &Arc<Gate>, counts: &Arc<Counts>) -> Result<Self> {
        let owner = Self(Arc::pin_init(
            pin_init!(Closer {
                work <- new_work!("CastKMS test grant shutdown"),
                started <- Completion::new(),
                registry: registry.clone(),
                gate: gate.clone(),
                counts: counts.clone(),
                revokes_seen: AtomicU32::new(u32::MAX),
            }),
            GFP_KERNEL,
        )?);
        check(workqueue::system_dfl().enqueue(owner.0.clone()).is_ok())?;
        owner.0.started.wait_for_completion();
        Ok(owner)
    }

    fn finish(&self) {
        self.0.gate.proceed.complete_all();
        self.0.work.flush();
    }
}

impl Drop for Closing {
    fn drop(&mut self) {
        self.finish();
    }
}

#[kunit_tests(rust_castkms_authority_grants)]
mod cases {
    use super::*;

    #[test]
    fn overlapping_shutdown_calls_observe_completed_provider_cleanup() -> Result {
        for _ in 0..32 {
            let registry = Registry::new()?;
            let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
            let gate = Arc::pin_init(
                pin_init!(Gate {
                    entered <- Completion::new(),
                    proceed <- Completion::new(),
                }),
                GFP_KERNEL,
            )?;
            let authority = Authority::new(Arc::new(
                GatedPolicy {
                    gate: gate.clone(),
                    counts: counts.clone(),
                },
                GFP_KERNEL,
            )?)?;
            let mut grant = GatedGrant {
                authority,
                registration: None,
                gate: gate.clone(),
            };
            grant.registration = Some(registry.register(&grant.authority.revocation())?);
            let first = Closing::start(&registry, &gate, &counts)?;
            gate.entered.wait_for_completion();
            let second = Closing::start(&registry, &gate, &counts)?;
            first.finish();
            second.finish();
            check(first.0.revokes_seen.load(Ordering::Relaxed) == 1)?;
            check(second.0.revokes_seen.load(Ordering::Relaxed) == 1)?;
        }
        Ok(())
    }

    #[test]
    fn shutdown_revokes_before_releasing_retained_policy() -> Result {
        let registry = Registry::new()?;
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let authority = authority(&counts, None)?;
        let registration = registry.register(&authority.revocation())?;
        drop(authority);
        registry.close();
        registry.close();
        check(counts.revokes.load(Ordering::Relaxed) == 1)?;
        check(counts.releases.load(Ordering::Relaxed) == 0)?;
        drop(registration);
        check(counts.releases.load(Ordering::Relaxed) == 1)?;
        check(counts.revokes.load(Ordering::Relaxed) == 1)?;
        Ok(())
    }

    #[test]
    fn closing_one_registration_leaves_other_grants_usable() -> Result {
        let registry = Registry::new()?;
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let first = authority(&counts, None)?;
        let second = authority(&counts, None)?;
        let registration = registry.register(&first.revocation())?;
        let _second_registration = registry.register(&second.revocation())?;
        drop(registration);
        check(first.cleanup_done())?;
        check(!second.is_revoked())?;
        registry.close();
        check(second.cleanup_done())?;
        check(counts.revokes.load(Ordering::Relaxed) == 2)?;
        Ok(())
    }

    #[test]
    fn failed_duplicate_registration_does_not_revoke_the_grant() -> Result {
        let registry = Registry::new()?;
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let authority = authority(&counts, None)?;
        let _registration = registry.register(&authority.revocation())?;
        check(matches!(
            registry.register(&authority.revocation()),
            Err(EEXIST)
        ))?;
        check(!authority.is_revoked())?;
        Ok(())
    }

    #[test]
    fn an_empty_shutdown_permanently_closes_registration() -> Result {
        let registry = Registry::new()?;
        registry.close();
        registry.close();
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let authority = authority(&counts, None)?;
        check(matches!(
            registry.register(&authority.revocation()),
            Err(ENODEV)
        ))?;
        check(!authority.is_revoked())?;
        Ok(())
    }

    #[test]
    fn returned_registration_credit_is_available_without_waiting_for_references() -> Result {
        let registry = Registry::new()?;
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let mut registrations = KVec::new();
        for _ in 0..MAX_GRANTS {
            let authority = authority(&counts, None)?;
            registrations.push(registry.register(&authority.revocation())?, GFP_KERNEL)?;
        }
        let extra = authority(&counts, None)?;
        check(matches!(registry.register(&extra.revocation()), Err(EBUSY)))?;
        check(!extra.is_revoked())?;
        let retired = registrations.pop().ok_or(EINVAL)?;
        let retained = retired.entry.revocation.clone();
        drop(retired);
        check(retained.cleanup_done())?;
        let _replacement = registry.register(&extra.revocation())?;
        registry.close();
        check(extra.cleanup_done())?;
        check(counts.revokes.load(Ordering::Relaxed) == MAX_GRANTS as u32 + 1)?;
        Ok(())
    }

    #[test]
    fn shutdown_callbacks_run_after_registration_is_closed_and_unlocked() -> Result {
        let registry = Registry::new()?;
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let other = authority(&counts, None)?;
        let probe = Probe {
            registry: registry.clone(),
            other: other.revocation(),
        };
        let checked = authority(&counts, Some(probe))?;
        let _registration = registry.register(&checked.revocation())?;
        registry.close();
        check(counts.registration_denied.load(Ordering::Relaxed))?;
        check(checked.cleanup_done())?;
        check(!other.is_revoked())?;
        Ok(())
    }
}
