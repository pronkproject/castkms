// SPDX-License-Identifier: GPL-2.0-only

//! Virtual monitor publication and terminal shutdown.

use super::*;
use crate::monitor::{Description, Monitor};
use kernel::drm::kms::connector::Status;

#[kunit_tests(rust_castkms_monitor)]
mod cases {
    use super::*;

    #[test]
    fn descriptions_change_connection_status() -> Result {
        let monitor = Monitor::new()?;
        check(monitor.status() == Status::Connected)?;
        monitor.publish(Description::Disconnected)?;
        check(monitor.status() == Status::Disconnected)?;
        monitor.publish(Description::Attached(None))?;
        check(monitor.status() == Status::Connected)?;
        monitor.publish(Description::Fallback)?;
        check(monitor.status() == Status::Connected)
    }

    #[test]
    fn shutdown_rejects_later_publication() -> Result {
        let monitor = Monitor::new()?;
        monitor.close();
        check(monitor.status() == Status::Disconnected)?;
        check(monitor.publish(Description::Fallback) == Err(ENODEV))?;
        monitor.close();
        check(monitor.status() == Status::Disconnected)
    }
}
