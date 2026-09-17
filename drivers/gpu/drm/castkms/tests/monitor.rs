// SPDX-License-Identifier: GPL-2.0-only

//! Virtual monitor publication and terminal shutdown.

use super::*;
use crate::monitor::Monitor;
use kernel::drm::kms::connector::Status;

#[kunit_tests(rust_castkms_monitor)]
mod cases {
    use super::*;

    #[test]
    fn reservation_preserves_disconnection_until_publication() -> Result {
        let driver = CastKms::new(c"castkms-monitor-pending")?;
        let device = driver._display.registration_guard().ok_or(ENODEV)?;
        let pending = device.monitor.reserve(&device)?;
        check(device.monitor.status() == Status::Disconnected)?;
        check(matches!(device.monitor.reserve(&device), Err(EBUSY)))?;
        drop(pending);
        check(device.monitor.status() == Status::Disconnected)?;
        let pending = device.monitor.reserve(&device)?;
        let control = pending.publish()?;
        check(device.monitor.status() == Status::Disconnected)?;
        drop(control);
        let pending = device.monitor.reserve(&device)?;
        device.monitor.close();
        check(matches!(pending.publish(), Err(ENODEV)))?;
        check(device.monitor.status() == Status::Disconnected)
    }

    #[test]
    fn only_explicit_attachment_connects_the_monitor() -> Result {
        let driver = CastKms::new(c"castkms-monitor-control")?;
        let device = driver._display.registration_guard().ok_or(ENODEV)?;
        check(device.monitor.status() == Status::Disconnected)?;
        let control = device.monitor.acquire(&device)?;
        check(device.monitor.status() == Status::Disconnected)?;
        control.attach(None)?;
        check(device.monitor.status() == Status::Connected)?;
        control.detach()?;
        check(device.monitor.status() == Status::Disconnected)?;
        drop(control);
        check(device.monitor.status() == Status::Disconnected)
    }

    #[test]
    fn control_is_exclusive_and_shutdown_is_terminal() -> Result {
        let driver = CastKms::new(c"castkms-monitor-exclusion")?;
        let device = driver._display.registration_guard().ok_or(ENODEV)?;
        let other = Monitor::new()?;
        check(matches!(other.acquire(&device), Err(EINVAL)))?;
        let control = device.monitor.acquire(&device)?;
        check(matches!(device.monitor.acquire(&device), Err(EBUSY)))?;
        device.monitor.close();
        check(device.monitor.status() == Status::Disconnected)?;
        check(control.attach(None) == Err(ENODEV))?;
        drop(control);
        check(matches!(device.monitor.acquire(&device), Err(ENODEV)))
    }
}
