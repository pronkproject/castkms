// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::drm::kms::testing::TestDevice;

#[track_caller]
fn check(condition: bool) -> Result {
    if condition {
        Ok(())
    } else {
        let location = core::panic::Location::caller();
        pr_err!(
            "Master file check failed at {}:{}\n",
            location.file(),
            location.line()
        );
        Err(EINVAL)
    }
}

#[kunit_tests(rust_drm_private_master_files)]
mod cases {
    use super::*;

    #[test]
    fn creation_and_close_follow_native_master_callbacks() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-private-master-file", None)?;
        let fixture = TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let file = fixture.master_file()?;
        let snapshot = file.file().master_snapshot().ok_or(EINVAL)?;
        check(snapshot.was_current())?;
        check(!snapshot.master().is_lessee())?;
        check(counts.master_sets.load(Ordering::Relaxed) == 1)?;
        check(counts.master_drops.load(Ordering::Relaxed) == 0)?;
        {
            let access = snapshot.master().lock_current().ok_or(EINVAL)?;
            check(access.is_master_file(file.file()))?;
            check(access.holds_object(fixture.crtc()?))?;
            // Private mode setup reserves connector IDs without publishing their objects.
            check(!access.holds_object(fixture.connector()?))?;
        }
        drop(file);
        check(snapshot.master().lock_current().is_none())?;
        check(counts.master_drops.load(Ordering::Relaxed) == 1)?;
        drop(snapshot);
        drop(fixture);
        check(counts.objects.load(Ordering::Relaxed) == 0)?;
        Ok(())
    }

    #[test]
    fn another_current_master_rejects_creation_without_a_callback() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-private-master-busy", None)?;
        let fixture = TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let first = fixture.master_file()?;
        let identity = first.file().associated_master().ok_or(EINVAL)?;
        check(matches!(fixture.master_file(), Err(EBUSY)))?;
        check(counts.master_sets.load(Ordering::Relaxed) == 1)?;
        check(counts.master_drops.load(Ordering::Relaxed) == 0)?;
        check(identity.lock_current().is_some())?;
        drop(first);
        let second = fixture.master_file()?;
        let replacement = second.file().associated_master().ok_or(EINVAL)?;
        check(identity != replacement)?;
        check(identity.lock_current().is_none())?;
        check(replacement.lock_current().is_some())?;
        check(counts.master_sets.load(Ordering::Relaxed) == 2)?;
        drop(second);
        check(counts.master_drops.load(Ordering::Relaxed) == 2)?;
        Ok(())
    }

    #[test]
    fn error_return_closes_the_master_before_fixture_shutdown() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-private-master-unwind", None)?;
        let fixture = TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let result = (|| -> Result {
            let _file = fixture.master_file()?;
            Err(EIO)
        })();
        check(result == Err(EIO))?;
        check(counts.master_sets.load(Ordering::Relaxed) == 1)?;
        check(counts.master_drops.load(Ordering::Relaxed) == 1)?;
        let file = fixture.master_file()?;
        check(
            file.file()
                .master_snapshot()
                .is_some_and(|state| state.was_current()),
        )?;
        Ok(())
    }
}
