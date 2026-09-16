// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::{
    drm::{
        constraints::{
            Description,
            Format,
            Size, //
        },
        fourcc, //
    },
    sync::Arc, //
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

// SAFETY: Backend destruction and callbacks reside in the kernel crate's LocalModule.
#[vtable]
unsafe impl Backend for TestBackend {}

fn entry(domain: &Domain, drops: &Arc<AtomicU32>) -> Result<ARef<Entry<TestBackend>>> {
    let size = Size::exact(128, 64);
    let description = Description::new(size, &[Format::new(7, fourcc::XRGB8888, 0, size)], &[])?;
    Entry::new(
        domain,
        9,
        &description,
        Arc::new(TestBackend(drops.clone()), GFP_KERNEL)?,
    )
}

#[kunit_tests(rust_drm_constraints_catalog)]
mod cases {
    use super::*;

    #[test]
    fn snapshot_retains_entries_after_catalog_closes_and_drops() -> Result {
        let drops = Arc::new(AtomicU32::new(0), GFP_KERNEL)?;
        let domain = Domain::new(2)?;
        let initial = entry(&domain, &drops)?;
        let target = entry(&domain, &drops)?;
        let catalog = Catalog::new(&domain, &initial, 2)?;
        catalog.add(&target)?;
        catalog.suggest(target.id())?;
        let snapshot = catalog.snapshot(0)?;
        let info = snapshot.info();
        assert_eq!(info.count, 2);
        assert_eq!(info.selected_id, initial.id());
        assert_eq!(info.suggested_id, target.id());
        catalog.close();
        assert!(matches!(catalog.snapshot(0), Err(ESTALE)));
        assert_eq!(catalog.selected().id(), initial.id());
        drop(catalog);
        drop(initial);
        drop(target);
        drop(domain);
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        assert_eq!(snapshot.entries().len(), 2);
        assert!(snapshot.entries().all(|offer| offer.selectable));
        assert_eq!(
            snapshot.entries().next().unwrap().entry.id(),
            info.selected_id
        );
        drop(snapshot);
        assert_eq!(drops.load(Ordering::Relaxed), 2);
        Ok(())
    }

    #[test]
    fn withdrawn_offer_keeps_immutable_snapshot_meaning() -> Result {
        let drops = Arc::new(AtomicU32::new(0), GFP_KERNEL)?;
        let domain = Domain::new(2)?;
        let initial = entry(&domain, &drops)?;
        let target = entry(&domain, &drops)?;
        let catalog = Catalog::new(&domain, &initial, 2)?;
        catalog.add(&target)?;
        let old = catalog.snapshot(0)?;
        catalog.withdraw(target.id())?;
        assert!(matches!(
            catalog.snapshot(old.info().generation),
            Err(ESTALE)
        ));
        let current = catalog.snapshot(0)?;
        assert!(old.entries().all(|offer| offer.selectable));
        assert!(!current.entries().nth(1).unwrap().selectable);
        catalog.forget(target.id())?;
        drop(target);
        assert_eq!(catalog.snapshot(0)?.info().count, 1);
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        drop(old);
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        drop(current);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        Ok(())
    }

    #[test]
    fn native_catalog_enforces_bound_and_domain() -> Result {
        let drops = Arc::new(AtomicU32::new(0), GFP_KERNEL)?;
        let domain = Domain::new(2)?;
        let other = Domain::new(1)?;
        let initial = entry(&domain, &drops)?;
        let target = entry(&domain, &drops)?;
        let foreign = entry(&other, &drops)?;
        let catalog = Catalog::new(&domain, &initial, 1)?;
        assert_eq!(catalog.add(&target), Err(ENOSPC));
        assert_eq!(catalog.add(&foreign), Err(EINVAL));
        assert_eq!(catalog.forget(initial.id()), Err(EBUSY));
        let before = catalog.snapshot(0)?.info();
        catalog.suggest(0)?;
        assert_eq!(catalog.snapshot(before.generation)?.info(), before);
        Ok(())
    }
}
