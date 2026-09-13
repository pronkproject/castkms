// SPDX-License-Identifier: GPL-2.0-only

//! Allocation credit follows native storage, not the caller's original handle.

use super::*;
use kernel::page::PAGE_SIZE;

#[kunit_tests(rust_castkms_gem_budget)]
mod cases {
    use super::*;

    #[test]
    fn invalid_sizes_leave_the_budget_available() -> Result {
        let fixture = Fixture::new()?;
        let device = fixture.drm.device();
        let budget = gem::budget::Budget::new(PAGE_SIZE)?;
        check(matches!(gem::budget::Budget::new(0), Err(EINVAL)))?;
        for (size, error) in [(0, EINVAL), (PAGE_SIZE - 1, EINVAL), (PAGE_SIZE + 1, E2BIG)] {
            check(
                matches!(gem::Object::new_budgeted(device, &budget, size), Err(e) if e == error),
            )?;
        }
        let _object = gem::Object::new_budgeted(device, &budget, PAGE_SIZE)?;
        Ok(())
    }

    #[test]
    fn retained_object_and_mapping_keep_the_allocation_charged() -> Result {
        let fixture = Fixture::new()?;
        let device = fixture.drm.device();
        let budget = gem::budget::Budget::new(PAGE_SIZE)?;
        let object = gem::Object::new_budgeted(device, &budget, PAGE_SIZE)?;
        let retained = object.clone();
        let mapping = object.owned_vmap::<0>()?;
        drop(object);
        check(matches!(
            gem::Object::new_budgeted(device, &budget, PAGE_SIZE),
            Err(EBUSY)
        ))?;
        drop(retained);
        check(matches!(
            gem::Object::new_budgeted(device, &budget, PAGE_SIZE),
            Err(EBUSY)
        ))?;
        drop(mapping);
        let _replacement = gem::Object::new_budgeted(device, &budget, PAGE_SIZE)?;
        Ok(())
    }

    #[test]
    fn retired_allocations_share_credit_without_blocking_another_budget() -> Result {
        let fixture = Fixture::new()?;
        let device = fixture.drm.device();
        let budget = gem::budget::Budget::new(3 * PAGE_SIZE)?;
        let other = gem::budget::Budget::new(PAGE_SIZE)?;
        let first = gem::Object::new_budgeted(device, &budget, 2 * PAGE_SIZE)?;
        let last = gem::Object::new_budgeted(device, &budget, PAGE_SIZE)?;
        check(matches!(
            gem::Object::new_budgeted(device, &budget, PAGE_SIZE),
            Err(EBUSY)
        ))?;
        let _independent = gem::Object::new_budgeted(device, &other, PAGE_SIZE)?;
        drop(first);
        check(matches!(
            gem::Object::new_budgeted(device, &budget, 3 * PAGE_SIZE),
            Err(EBUSY)
        ))?;
        let _replacement = gem::Object::new_budgeted(device, &budget, 2 * PAGE_SIZE)?;
        drop(last);
        let _last = gem::Object::new_budgeted(device, &budget, PAGE_SIZE)?;
        Ok(())
    }

    #[cfg(CONFIG_DRM_CLIENT)]
    #[test]
    fn native_export_retains_credit_after_the_original_object_is_dropped() -> Result {
        use kernel::drm::gem::IntoGEMObject;

        // This private fixture exports once, without a file handle or a second export cache.
        struct Export(*mut kernel::bindings::dma_buf);
        impl Drop for Export {
            fn drop(&mut self) {
                // SAFETY: The successful native export transfers one owned file reference.
                unsafe { kernel::bindings::fput((*self.0).file) };
            }
        }

        with_exporter(|fixture| {
            let device = fixture.drm.device();
            let budget = gem::budget::Budget::new(PAGE_SIZE)?;
            let object = gem::Object::new_budgeted(device, &budget, PAGE_SIZE)?;
            // SAFETY: This fresh native object is live and has no existing export or handles.
            // The helper retains both object and device independently of this Rust handle.
            let raw = kernel::error::from_err_ptr(unsafe {
                kernel::bindings::drm_gem_prime_export(
                    object.as_raw(),
                    kernel::bindings::O_RDWR as i32,
                )
            })?;
            let export = Export(raw);
            drop(object);
            check(matches!(
                gem::Object::new_budgeted(device, &budget, PAGE_SIZE),
                Err(EBUSY)
            ))?;
            drop(export);
            // SAFETY: No locks are held; this kernel thread can drain its deferred file release.
            unsafe { kernel::bindings::flush_delayed_fput() };
            let _replacement = gem::Object::new_budgeted(device, &budget, PAGE_SIZE)?;
            Ok(())
        })
    }
}
