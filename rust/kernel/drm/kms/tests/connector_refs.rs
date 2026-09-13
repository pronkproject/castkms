// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Owned connector references retain native mode configuration through final cleanup.

use super::*;
use crate::{
    drm::kms::{
        connector::{
            AsRawConnector,
            OpaqueConnector,
            RawConnector, //
        },
        testing::TestDevice, //
    },
    sync::aref::ARef, //
};

#[track_caller]
fn check(condition: bool) -> Result {
    if condition {
        Ok(())
    } else {
        let location = core::panic::Location::caller();
        pr_err!(
            "Connector reference check failed at {}:{}\n",
            location.file(),
            location.line()
        );
        Err(EINVAL)
    }
}

fn typed_reference(counts: &Arc<Counts>, fail_after_shutdown: bool) -> Result {
    let parent = faux::Registration::new(c"rust-connector-ref", None)?;
    let fixture = TestDevice::new(allocate(parent.as_ref(), counts, false)?)?;
    let baseline = device_references(fixture.device());
    let connector: ARef<connector::Connector<TestConnector>> = fixture.connector()?.into();
    let clone = connector.clone();
    check(device_references(fixture.device()) == baseline + 2)?;
    drop(connector);
    check(device_references(fixture.device()) == baseline + 1)?;
    drop(fixture);
    check(counts.objects.load(Ordering::Relaxed) != 0)?;
    check(clone.mask() != 0)?;
    if fail_after_shutdown {
        return Err(EIO);
    }
    drop(clone);
    check(counts.objects.load(Ordering::Relaxed) == 0)
}

fn opaque_reference(counts: &Arc<Counts>, fail_after_shutdown: bool) -> Result {
    let parent = faux::Registration::new(c"rust-opaque-connector-ref", None)?;
    let fixture = TestDevice::new(allocate(parent.as_ref(), counts, false)?)?;
    let raw = fixture.connector()?.as_raw();
    // SAFETY: The fixture retains the initialized connector and its nominated driver.
    let opaque = unsafe { OpaqueConnector::<TestDriver>::from_raw(raw) };
    let baseline = device_references(fixture.device());
    let connector: ARef<OpaqueConnector<TestDriver>> = opaque.into();
    let clone = connector.clone();
    check(device_references(fixture.device()) == baseline + 2)?;
    drop(connector);
    check(device_references(fixture.device()) == baseline + 1)?;
    drop(fixture);
    check(counts.objects.load(Ordering::Relaxed) != 0)?;
    check(clone.mask() != 0)?;
    if fail_after_shutdown {
        return Err(EIO);
    }
    drop(clone);
    check(counts.objects.load(Ordering::Relaxed) == 0)
}

#[kunit_tests(rust_drm_connector_refs)]
mod cases {
    use super::*;

    #[test]
    fn typed_reference_owns_its_device_until_the_last_clone_drops() -> Result {
        typed_reference(&Arc::new(Counts::default(), GFP_KERNEL)?, false)
    }

    #[test]
    fn opaque_reference_independently_retains_mode_configuration() -> Result {
        opaque_reference(&Arc::new(Counts::default(), GFP_KERNEL)?, false)
    }

    #[test]
    fn errors_release_retained_connectors_before_reporting_failure() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        check(typed_reference(&counts, true) == Err(EIO))?;
        check(counts.objects.load(Ordering::Relaxed) == 0)?;
        check(opaque_reference(&counts, true) == Err(EIO))?;
        check(counts.objects.load(Ordering::Relaxed) == 0)
    }
}
