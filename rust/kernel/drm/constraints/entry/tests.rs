// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::drm::{
    constraints::{
        Format,
        Size, //
    },
    fourcc, //
};
use core::sync::atomic::{
    AtomicU32,
    Ordering, //
};

struct TestBackend(Arc<AtomicU32>);

impl Drop for TestBackend {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

// SAFETY: Backend destruction and entry callbacks reside in the kernel crate's LocalModule.
#[vtable]
unsafe impl Backend for TestBackend {}

fn description() -> Result<ARef<Description>> {
    let size = Size::exact(128, 64);
    Description::new(size, &[Format::new(7, fourcc::XRGB8888, 0, size)], &[], &[])
}

fn backend(drops: &Arc<AtomicU32>) -> Result<Arc<TestBackend>> {
    Ok(Arc::new(TestBackend(drops.clone()), GFP_KERNEL)?)
}

#[kunit_tests(rust_drm_constraints_entry)]
mod cases {
    use super::*;

    #[test]
    fn opaque_reference_retains_typed_backend() -> Result {
        let drops = Arc::new(AtomicU32::new(0), GFP_KERNEL)?;
        let domain = Domain::new(1)?;
        let description = description()?;
        let entry = Entry::new(&domain, 9, &description, backend(&drops)?)?;
        let retained = ARef::<OpaqueEntry>::from(&**entry);
        assert_eq!(retained.id(), entry.id());
        drop(entry);
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        assert!(matches!(
            OpaqueEntry::new_stateless(&domain, 9, &description),
            Err(ENOSPC)
        ));
        assert!(retained.in_domain(&domain));
        assert_eq!(retained.description().output().minimum(), (128, 64));
        drop(retained);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        Ok(())
    }

    #[test]
    fn stateless_reference_retains_description_and_domain() -> Result {
        let domain = Domain::new(1)?;
        let description = description()?;
        let entry = OpaqueEntry::new_stateless(&domain, 9, &description)?;
        assert_ne!(entry.id(), 0);
        assert_eq!(entry.crtc_id(), 9);
        assert!(entry.in_domain(&domain));
        drop(domain);
        drop(description);
        let retained = entry.clone();
        drop(entry);
        assert_eq!(retained.description().output().maximum(), (128, 64));
        Ok(())
    }

    #[test]
    fn entry_retains_domain_description_and_backend() -> Result {
        let drops = Arc::new(AtomicU32::new(0), GFP_KERNEL)?;
        let domain = Domain::new(2)?;
        let description = description()?;
        let entry = Entry::new(&domain, 9, &description, backend(&drops)?)?;
        assert!(entry.in_domain(&domain));
        assert_eq!(entry.crtc_id(), 9);
        assert_ne!(entry.id(), 0);
        drop(domain);
        drop(description);
        let retained = entry.clone();
        drop(entry);
        assert_eq!(retained.description().output().minimum(), (128, 64));
        assert_eq!(retained.backend().0.load(Ordering::Relaxed), 0);
        drop(retained);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        Ok(())
    }

    #[test]
    fn retained_entries_charge_capacity_without_reusing_ids() -> Result {
        let drops = Arc::new(AtomicU32::new(0), GFP_KERNEL)?;
        let domain = Domain::new(1)?;
        let description = description()?;
        let entry = Entry::new(&domain, 9, &description, backend(&drops)?)?;
        let id = entry.id();
        let retained = entry.clone();
        drop(entry);
        assert!(matches!(
            Entry::new(&domain, 9, &description, backend(&drops)?),
            Err(ENOSPC)
        ));
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        drop(retained);
        assert_eq!(drops.load(Ordering::Relaxed), 2);
        let next = Entry::new(&domain, 9, &description, backend(&drops)?)?;
        assert!(next.id() > id);
        drop(next);
        assert_eq!(drops.load(Ordering::Relaxed), 3);
        Ok(())
    }

    #[test]
    fn same_number_in_foreign_domain_is_not_membership() -> Result {
        let drops = Arc::new(AtomicU32::new(0), GFP_KERNEL)?;
        let local = Domain::new(1)?;
        let foreign = Domain::new(1)?;
        let description = description()?;
        let first = Entry::new(&local, 9, &description, backend(&drops)?)?;
        let second = Entry::new(&foreign, 9, &description, backend(&drops)?)?;
        assert_eq!(first.id(), second.id());
        assert!(!first.in_domain(&foreign));
        assert!(!second.in_domain(&local));
        Ok(())
    }
}
