// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::{
    drm::{
        constraints::{
            Backend,
            Description,
            Entry,
            Format,
            Property,
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

#[kunit_tests(rust_drm_constraints_list)]
mod cases {
    use super::*;

    #[test]
    fn maximum_snapshot_encodes_into_independent_large_storage() -> Result {
        let drops = Arc::new(AtomicU32::new(0), GFP_KERNEL)?;
        let size = Size::exact(128, 64);
        let count = bindings::DRM_CONSTRAINTS_MAX_ENTRIES;
        let mut formats = KVVec::new();
        for index in 0..bindings::DRM_CONSTRAINTS_MAX_FORMATS {
            formats.push(
                Format::new(
                    7 + index / 256,
                    fourcc::XRGB8888,
                    (index % 256).into(),
                    size,
                ),
                GFP_KERNEL,
            )?;
        }
        let mut properties = KVec::new();
        for index in 0..bindings::DRM_CONSTRAINTS_MAX_PROPERTIES {
            properties.push(
                Property::unsigned_range(7, 100 + index, 0, 65535),
                GFP_KERNEL,
            )?;
        }
        // Metadata capacity only: these alternatives are not attached to a KMS device.
        let description = Description::new(size, &formats, &properties)?;
        let domain = Domain::new(count)?;
        let initial = Entry::new(
            &domain,
            9,
            &description,
            Arc::new(TestBackend(drops.clone()), GFP_KERNEL)?,
        )?;
        let list = List::new(&domain, &initial, count)?;
        for _ in 1..count {
            let next = Entry::new(
                &domain,
                9,
                &description,
                Arc::new(TestBackend(drops.clone()), GFP_KERNEL)?,
            )?;
            list.add(&next)?;
        }
        let snapshot = list.snapshot(0)?;
        let bytes = snapshot.encode()?;
        let description_size =
            core::mem::size_of::<bindings::drm_constraints_encoded_description>()
                + core::mem::size_of::<bindings::drm_constraints_encoded_output>()
                + formats.len() * core::mem::size_of::<bindings::drm_constraints_encoded_format>()
                + properties.len()
                    * core::mem::size_of::<bindings::drm_constraints_encoded_property>();
        let expected = core::mem::size_of::<bindings::drm_constraints_encoded_list>()
            + count as usize
                * (core::mem::size_of::<bindings::drm_constraints_encoded_entry>()
                    + description_size);
        assert_eq!(bytes.len(), expected);
        assert!(bytes.len() > 4 * 1024 * 1024);
        assert!(bytes.len() <= bindings::DRM_CONSTRAINTS_ENCODING_MAX_SIZE as usize);
        list.close();
        drop(snapshot);
        drop(list);
        drop(initial);
        drop(description);
        drop(domain);
        assert_eq!(drops.load(Ordering::Relaxed), count);
        let length = core::mem::offset_of!(bindings::drm_constraints_encoded_list, length);
        assert_eq!(
            u32::from_ne_bytes(bytes[length..length + 4].try_into().unwrap()) as usize,
            expected
        );
        let last = bytes.len() - core::mem::size_of::<bindings::drm_constraints_encoded_property>();
        let property_id =
            last + core::mem::offset_of!(bindings::drm_constraints_encoded_property, property_id);
        assert_eq!(
            u32::from_ne_bytes(bytes[property_id..property_id + 4].try_into().unwrap()),
            99 + bindings::DRM_CONSTRAINTS_MAX_PROPERTIES
        );
        Ok(())
    }

    #[test]
    fn stateless_and_typed_backends_share_one_retained_list() -> Result {
        let drops = Arc::new(AtomicU32::new(0), GFP_KERNEL)?;
        let domain = Domain::new(2)?;
        let target = entry(&domain, &drops)?;
        let initial = OpaqueEntry::new_stateless(&domain, 9, target.description())?;
        let list = List::new(&domain, &initial, 2)?;
        list.add(&target)?;
        let snapshot = list.snapshot(0)?;
        let retained = list.lookup(target.id())?;
        assert_eq!(list.selected().id(), initial.id());
        assert!(retained.in_domain(&domain));
        list.withdraw(target.id())?;
        list.forget(target.id())?;
        list.close();
        drop(list);
        drop(initial);
        drop(target);
        drop(domain);
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        assert_eq!(snapshot.entries().len(), 2);
        assert!(snapshot.entries().all(|offer| offer.entry.crtc_id() == 9));
        drop(snapshot);
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        drop(retained);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        Ok(())
    }

    #[test]
    fn snapshot_retains_entries_after_list_closes_and_drops() -> Result {
        let drops = Arc::new(AtomicU32::new(0), GFP_KERNEL)?;
        let domain = Domain::new(2)?;
        let initial = entry(&domain, &drops)?;
        let target = entry(&domain, &drops)?;
        let list = List::new(&domain, &initial, 2)?;
        list.add(&target)?;
        list.suggest(target.id())?;
        let snapshot = list.snapshot(0)?;
        let info = snapshot.info();
        assert_eq!(info.count, 2);
        assert_eq!(info.selected_id, initial.id());
        assert_eq!(info.suggested_id, target.id());
        list.close();
        assert!(matches!(list.snapshot(0), Err(ESTALE)));
        assert_eq!(list.selected().id(), initial.id());
        drop(list);
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
        let list = List::new(&domain, &initial, 2)?;
        list.add(&target)?;
        let old = list.snapshot(0)?;
        list.withdraw(target.id())?;
        assert!(matches!(list.snapshot(old.info().generation), Err(ESTALE)));
        let current = list.snapshot(0)?;
        assert!(old.entries().all(|offer| offer.selectable));
        assert!(!current.entries().nth(1).unwrap().selectable);
        list.forget(target.id())?;
        drop(target);
        assert_eq!(list.snapshot(0)?.info().count, 1);
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        drop(old);
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        drop(current);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        Ok(())
    }

    #[test]
    fn native_list_enforces_bound_and_domain() -> Result {
        let drops = Arc::new(AtomicU32::new(0), GFP_KERNEL)?;
        let domain = Domain::new(2)?;
        let other = Domain::new(1)?;
        let initial = entry(&domain, &drops)?;
        let target = entry(&domain, &drops)?;
        let foreign = entry(&other, &drops)?;
        let list = List::new(&domain, &initial, 1)?;
        assert_eq!(list.add(&target), Err(ENOSPC));
        assert_eq!(list.add(&foreign), Err(EINVAL));
        assert_eq!(list.forget(initial.id()), Err(EBUSY));
        let before = list.snapshot(0)?.info();
        list.suggest(0)?;
        assert_eq!(list.snapshot(before.generation)?.info(), before);
        Ok(())
    }

    #[test]
    fn identity_lookup_retains_typed_backend_without_reserving_offer() -> Result {
        let drops = Arc::new(AtomicU32::new(0), GFP_KERNEL)?;
        let domain = Domain::new(2)?;
        let initial = entry(&domain, &drops)?;
        let target = entry(&domain, &drops)?;
        let list = List::new(&domain, &initial, 2)?;
        assert!(matches!(list.lookup(0), Err(EINVAL)));
        assert!(matches!(list.lookup(target.id()), Err(ESTALE)));
        list.add(&target)?;
        let retained = list.lookup(target.id())?;
        assert_eq!(retained.id(), target.id());
        assert!(retained.in_domain(&domain));
        list.withdraw(target.id())?;
        assert!(matches!(list.lookup(target.id()), Err(ESTALE)));
        list.forget(target.id())?;
        drop(target);
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        assert_eq!(retained.description().output().minimum(), (128, 64));
        drop(retained);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        list.withdraw(initial.id())?;
        assert_eq!(list.lookup(initial.id())?.id(), initial.id());
        list.close();
        assert!(matches!(list.lookup(initial.id()), Err(ESTALE)));
        Ok(())
    }

    #[test]
    fn encoded_bytes_keep_metadata_without_retaining_backends() -> Result {
        let drops = Arc::new(AtomicU32::new(0), GFP_KERNEL)?;
        let domain = Domain::new(2)?;
        let initial = entry(&domain, &drops)?;
        let target = entry(&domain, &drops)?;
        let list = List::new(&domain, &initial, 2)?;
        list.add(&target)?;
        list.suggest(target.id())?;
        let snapshot = list.snapshot(0)?;
        let info = snapshot.info();
        let bytes = snapshot.encode()?;
        assert!(bytes.len() <= bindings::DRM_CONSTRAINTS_ENCODING_MAX_SIZE as usize);
        assert!(bytes.len() >= core::mem::size_of::<bindings::drm_constraints_encoded_list>());
        let generation = core::mem::offset_of!(bindings::drm_constraints_encoded_list, generation);
        let selected = core::mem::offset_of!(bindings::drm_constraints_encoded_list, selected_id);
        let suggested = core::mem::offset_of!(bindings::drm_constraints_encoded_list, suggested_id);
        assert_eq!(
            u64::from_ne_bytes(bytes[generation..generation + 8].try_into().unwrap()),
            info.generation
        );
        assert_eq!(
            u64::from_ne_bytes(bytes[selected..selected + 8].try_into().unwrap()),
            info.selected_id
        );
        assert_eq!(
            u64::from_ne_bytes(bytes[suggested..suggested + 8].try_into().unwrap()),
            info.suggested_id
        );
        list.close();
        assert_eq!(snapshot.encode()?.as_slice(), bytes.as_slice());
        drop(snapshot);
        drop(list);
        drop(initial);
        drop(target);
        drop(domain);
        assert_eq!(drops.load(Ordering::Relaxed), 2);
        assert_eq!(
            u64::from_ne_bytes(bytes[selected..selected + 8].try_into().unwrap()),
            info.selected_id
        );
        Ok(())
    }
}
