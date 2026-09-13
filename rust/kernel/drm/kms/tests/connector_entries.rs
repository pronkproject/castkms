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
            "Connector entry check failed at {}:{}\n",
            location.file(),
            location.line()
        );
        Err(EINVAL)
    }
}

#[kunit_tests(rust_drm_private_connector_entries)]
mod cases {
    use super::*;

    #[test]
    fn native_access_tracks_the_private_connector_entry() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-private-connector", None)?;
        let fixture = TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let file = fixture.master_file()?;
        let master = file.file().associated_master().ok_or(EINVAL)?;
        let visible = || -> Result<bool> {
            let access = master.lock_current().ok_or(EINVAL)?;
            Ok(access.holds_object(fixture.connector()?))
        };
        check(!visible()?)?;
        let entry = fixture.publish_connector_identity()?;
        check(visible()?)?;
        check(matches!(fixture.publish_connector_identity(), Err(EEXIST)))?;
        check(visible()?)?;
        drop(entry);
        check(!visible()?)?;
        let replacement = fixture.publish_connector_identity()?;
        check(visible()?)?;
        drop(replacement);
        check(!visible()?)?;
        Ok(())
    }

    #[test]
    fn published_connector_identity_does_not_cross_devices() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-foreign-connector", None)?;
        let first = TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let second = TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let _first_entry = first.publish_connector_identity()?;
        let _second_entry = second.publish_connector_identity()?;
        let file = first.master_file()?;
        let master = file.file().associated_master().ok_or(EINVAL)?;
        let access = master.lock_current().ok_or(EINVAL)?;
        check(access.holds_object(first.connector()?))?;
        check(!access.holds_object(second.connector()?))?;
        Ok(())
    }

    #[test]
    fn error_unwinding_removes_the_entry_without_freeing_its_id() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-connector-entry-unwind", None)?;
        let fixture = TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let result = (|| -> Result {
            let _entry = fixture.publish_connector_identity()?;
            Err(EIO)
        })();
        check(result == Err(EIO))?;
        let entry = fixture.publish_connector_identity()?;
        drop(entry);
        drop(fixture);
        check(counts.objects.load(Ordering::Relaxed) == 0)?;
        Ok(())
    }
}
