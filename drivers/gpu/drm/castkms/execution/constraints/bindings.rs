// SPDX-License-Identifier: GPL-2.0-only

//! Typed renderer resources indexed by exact native constraints identity.

use kernel::{
    drm::constraints::{
        Backend,
        Domain,
        Entry,
        OpaqueEntry, //
    },
    prelude::*,
    sync::{
        aref::ARef,
        Mutex, //
    }, //
};

const MAX_BINDINGS: usize = 64;

struct State<B: Backend> {
    closed: bool,
    entries: KVec<ARef<Entry<B>>>,
}

/// Bounded ownership for resolving an opaque atomic binding to its exact renderer resources.
///
/// This index neither publishes offers nor establishes readiness. Insert before native offer
/// publication; remove only after native withdrawal and forgetting. Call native list operations
/// without holding this index's lock. Native validation may resolve under its own list lock.
/// Accepted driver state independently retains the returned typed entry through publication.
#[pin_data]
pub(crate) struct Bindings<B: Backend> {
    domain: ARef<Domain>,
    crtc_id: u32,
    capacity: usize,
    #[pin]
    state: Mutex<State<B>>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl<B: Backend> Bindings<B> {
    pub(crate) fn new(
        domain: ARef<Domain>,
        crtc_id: u32,
        limit: usize,
    ) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {
            domain,
            crtc_id: if crtc_id == 0 { return Err(EINVAL) } else { crtc_id },
            capacity: if limit == 0 || limit > MAX_BINDINGS {
                return Err(EINVAL)
            } else {
                limit
            },
            state <- kernel::new_mutex!(State {
                closed: false,
                entries: KVec::with_capacity(limit, GFP_KERNEL)?,
            }),
        })
    }

    /// Retain an unpublished entry; failure consumes no caller-owned reference.
    pub(crate) fn insert(&self, entry: &Entry<B>) -> Result {
        if !entry.in_domain(&self.domain) || entry.crtc_id() != self.crtc_id {
            return Err(EINVAL);
        }
        let mut state = self.state.lock();
        if state.closed {
            return Err(ESHUTDOWN);
        }
        if state.entries.iter().any(|stored| stored.id() == entry.id()) {
            return Err(EEXIST);
        }
        if state.entries.len() == self.capacity {
            return Err(ENOSPC);
        }
        state.entries.push(ARef::from(entry), GFP_KERNEL)?;
        Ok(())
    }

    fn same(stored: &Entry<B>, entry: &OpaqueEntry) -> bool {
        core::ptr::eq(&**stored, entry)
    }

    /// Resolve pointer identity, never reinterpret an arbitrary provider's private data.
    pub(crate) fn resolve(&self, entry: &OpaqueEntry) -> Result<ARef<Entry<B>>> {
        let state = self.state.lock();
        if state.closed {
            return Err(ESHUTDOWN);
        }
        state
            .entries
            .iter()
            .find(|stored| Self::same(stored, entry))
            .cloned()
            .ok_or(ESTALE)
    }

    /// Transfer ownership out of the index for destruction outside caller-held locks.
    pub(crate) fn remove(&self, entry: &OpaqueEntry) -> Option<ARef<Entry<B>>> {
        let mut state = self.state.lock();
        let index = state
            .entries
            .iter()
            .position(|stored| Self::same(stored, entry))?;
        state.entries.remove(index).ok()
    }

    /// Exclude future resolution after native publication is closed or offers are retired.
    ///
    /// Independently retained accepted bindings survive. The returned resources must be dropped
    /// outside native DRM and provider locks; closure is neither retirement nor revocation.
    pub(crate) fn close(&self) -> KVec<ARef<Entry<B>>> {
        let mut state = self.state.lock();
        state.closed = true;
        core::mem::take(&mut state.entries)
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
#[kunit_tests(rust_castkms_constraints_bindings)]
mod tests {
    use super::*;
    use core::sync::atomic::{
        AtomicUsize,
        Ordering, //
    };
    use kernel::{
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

    struct Renderer(Arc<AtomicUsize>);

    // SAFETY: The local module retains the renderer's destructor and atomic bookkeeping.
    #[vtable]
    unsafe impl Backend for Renderer {}

    impl Drop for Renderer {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn entry(
        domain: &Domain,
        crtc: u32,
        retired: &Arc<AtomicUsize>,
    ) -> Result<ARef<Entry<Renderer>>> {
        let size = Size::new(1, 1, 1920, 1080);
        let description =
            Description::new(size, &[Format::implicit(7, fourcc::XRGB8888, size)], &[])?;
        Entry::new(
            domain,
            crtc,
            &description,
            Arc::new(Renderer(retired.clone()), GFP_KERNEL)?,
        )
    }

    #[test]
    fn exact_identity_excludes_equal_numbers_from_other_domains() -> Result {
        let domain = Domain::new(4)?;
        let other = Domain::new(4)?;
        let retired = Arc::new(AtomicUsize::new(0), GFP_KERNEL)?;
        let selected = entry(&domain, 3, &retired)?;
        let foreign = entry(&other, 3, &retired)?;
        assert_eq!(selected.id(), foreign.id());
        let bindings = KBox::pin_init(Bindings::new(domain, 3, 2), GFP_KERNEL)?;
        bindings.insert(&selected)?;
        assert_eq!(bindings.insert(&foreign), Err(EINVAL));
        assert!(matches!(bindings.resolve(&foreign), Err(ESTALE)));
        assert!(bindings.remove(&foreign).is_none());
        let resolved = bindings.resolve(&selected)?;
        assert!(core::ptr::eq(&**resolved, &**selected));
        Ok(())
    }

    #[test]
    fn bounded_index_rejects_duplicate_and_foreign_output_entries() -> Result {
        let domain = Domain::new(4)?;
        let retired = Arc::new(AtomicUsize::new(0), GFP_KERNEL)?;
        let first = entry(&domain, 3, &retired)?;
        let second = entry(&domain, 3, &retired)?;
        let foreign = entry(&domain, 4, &retired)?;
        let bindings = KBox::pin_init(Bindings::new(domain, 3, 1), GFP_KERNEL)?;
        bindings.insert(&first)?;
        assert_eq!(bindings.insert(&first), Err(EEXIST));
        assert_eq!(bindings.insert(&second), Err(ENOSPC));
        assert_eq!(bindings.insert(&foreign), Err(EINVAL));
        assert!(bindings.remove(&first).is_some());
        assert!(matches!(bindings.resolve(&first), Err(ESTALE)));
        bindings.insert(&second)?;
        Ok(())
    }

    #[test]
    fn accepted_binding_survives_index_closure() -> Result {
        let domain = Domain::new(4)?;
        let retired = Arc::new(AtomicUsize::new(0), GFP_KERNEL)?;
        let selected = entry(&domain, 3, &retired)?;
        let bindings = KBox::pin_init(Bindings::new(domain, 3, 1), GFP_KERNEL)?;
        bindings.insert(&selected)?;
        let accepted = bindings.resolve(&selected)?;
        drop(selected);
        drop(bindings.close());
        assert_eq!(retired.load(Ordering::Relaxed), 0);
        assert!(matches!(bindings.resolve(&accepted), Err(ESHUTDOWN)));
        assert_eq!(bindings.insert(&accepted), Err(ESHUTDOWN));
        assert!(bindings.close().is_empty());
        drop(bindings);
        assert_eq!(accepted.backend().0.load(Ordering::Relaxed), 0);
        drop(accepted);
        assert_eq!(retired.load(Ordering::Relaxed), 1);
        Ok(())
    }

    #[test]
    fn invalid_bounds_are_rejected_before_index_allocation() -> Result {
        let domain = Domain::new(4)?;
        for (crtc, limit) in [(0, 1), (3, 0), (3, MAX_BINDINGS + 1)] {
            assert!(matches!(
                KBox::pin_init(
                    Bindings::<Renderer>::new(domain.clone(), crtc, limit),
                    GFP_KERNEL
                ),
                Err(EINVAL)
            ));
        }
        Ok(())
    }
}
