// SPDX-License-Identifier: GPL-2.0-only

//! Completed-image ownership can be checked without issuing either kind of grant.

use super::*;
use crate::{
    display_control::Target,
    host_compositor::{
        compose,
        layout::Layout,
        pool::Pool, //
    },
    host_snapshot::{
        Budget,
        Snapshot, //
    },
    image_access::Current, //
};
use kernel::drm::kms::testing::MasterFile;

fn target(fixture: &Fixture, file: &MasterFile<'_, Driver>) -> Result<Target> {
    let master = file.file().master_snapshot().ok_or(EINVAL)?;
    let guard = master.master().lock_current().ok_or(EACCES)?;
    Target::new(&guard, fixture.drm.crtc()?, fixture.drm.connector()?)
}

#[kunit_tests(rust_castkms_image_access)]
mod cases {
    use super::*;

    #[test]
    fn completed_images_and_private_copies_use_the_same_ownership_rules() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let target = target(&fixture, &file)?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(
            file.file().master_snapshot(),
        ))?;
        fixture.select(&fb, false, 0)?;
        let layout = Layout::new(640, 480)?;
        let pool = Pool::new(fixture.drm.device(), &fixture.host_budget, layout)?;
        let image = compose::current(&fixture.drm.device().output, &pool)?.ok_or(EINVAL)?;
        let snapshot = Snapshot::new(fixture.drm.device(), &Budget::new()?, &image)?;
        target.with_current(|control| {
            let current = Current::new(control)?;
            check(current.configuration() == image.configuration().ok_or(EINVAL)?)?;
            check(current.layout() == layout)?;
            current.check_image(&image)?;
            current.check_snapshot(&snapshot)
        })
    }

    #[test]
    fn display_control_cannot_construct_image_access_for_unowned_pixels() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        check(fixture.has_owner(None))?;
        let file = fixture.drm.master_file()?;
        let target = target(&fixture, &file)?;
        target.with_current(|control| check(matches!(Current::new(control), Err(EACCES))))
    }
}
