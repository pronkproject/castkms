// SPDX-License-Identifier: GPL-2.0-only

//! Historical snapshot attribution is checked against current display authority.

use super::*;
use crate::{
    capture::permission::Permission,
    host_compositor::{
        compose,
        layout::Layout,
        pool::Pool, //
    },
    host_snapshot::{
        Budget,
        Snapshot, //
    }, //
};
use kernel::drm::kms::testing::MasterFile;

fn permission(fixture: &Fixture, file: &MasterFile<'_, Driver>) -> Result<Permission> {
    let snapshot = file.file().master_snapshot().ok_or(EINVAL)?;
    let guard = snapshot.master().lock_current().ok_or(EACCES)?;
    Permission::new(&guard, fixture.drm.crtc()?, fixture.drm.connector()?)
}

fn snapshot(fixture: &Fixture, output: &Output) -> Result<Snapshot> {
    let pool = Pool::new(
        fixture.drm.device(),
        &fixture.host_budget,
        Layout::new(640, 480)?,
    )?;
    let image = compose::current(output, &pool)?.ok_or(EINVAL)?;
    Snapshot::new(fixture.drm.device(), &Budget::new()?, &image)
}

#[kunit_tests(rust_castkms_capture_snapshots)]
mod cases {
    use super::*;

    #[test]
    fn earlier_content_is_allowed_without_changing_its_serial() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let permission = permission(&fixture, &file)?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(
            file.file().master_snapshot(),
        ))?;
        fixture.select(&fb, false, 0)?;
        let copy = snapshot(&fixture, &fixture.drm.device().output)?;
        let serial = copy.content_serial();
        fixture.select(&fb, false, 0)?;
        let now = fixture
            .drm
            .device()
            .output
            .inspect(|scene| scene.and_then(|scene| scene.content_serial()));
        check(now != serial)?;
        permission.with_current(|current| current.check_snapshot(&copy))?;
        check(copy.content_serial() == serial)?;
        drop(file);
        check(matches!(
            permission.with_current(|current| current.check_snapshot(&copy)),
            Err(EACCES)
        ))?;
        Ok(())
    }

    #[test]
    fn equal_dimensions_do_not_reauthorize_an_old_display_interval() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let permission = permission(&fixture, &file)?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(
            file.file().master_snapshot(),
        ))?;
        fixture.select(&fb, false, 0)?;
        let copy = snapshot(&fixture, &fixture.drm.device().output)?;
        fixture.drm.update(|mut transaction| {
            transaction
                .as_mut()
                .set_crtc_config(fixture.drm.crtc()?, None)
        })?;
        fixture.select(&fb, false, 0)?;
        permission.with_current(|current| {
            check(current.layout() == copy.layout())?;
            check(current.configuration() != copy.configuration().ok_or(EINVAL)?)?;
            check(current.check_snapshot(&copy) == Err(EACCES))
        })?;
        Ok(())
    }

    #[test]
    fn copied_output_identity_cannot_be_substituted() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let permission = permission(&fixture, &file)?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(
            file.file().master_snapshot(),
        ))?;
        fixture.select(&fb, false, 0)?;
        let configuration =
            permission.with_current(|current| Ok(current.configuration().clone()))?;
        let other = kernel::sync::Arc::pin_init(Output::new(), GFP_KERNEL)?;
        other.publish_with_configuration(
            kernel::drm::preparation::Source::new(1)?,
            output::SceneUpdate::Replace(
                fixture.drm.device().output.inspect(|scene| scene.cloned()),
            ),
            Some(configuration),
        );
        let result = (|| {
            let copy = snapshot(&fixture, &other)?;
            permission.with_current(|current| check(current.check_snapshot(&copy) == Err(EACCES)))
        })();
        other.close();
        result
    }

    #[test]
    fn adopting_a_new_framebuffer_does_not_relabel_an_unowned_snapshot() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let unowned = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&unowned, false, 0)?;
        let copy = snapshot(&fixture, &fixture.drm.device().output)?;
        let file = fixture.drm.master_file()?;
        let permission = permission(&fixture, &file)?;
        let owned = fixture.framebuffer(provenance::Provenance::from_snapshot(
            file.file().master_snapshot(),
        ))?;
        fixture.select(&owned, false, 0)?;
        permission.with_current(|current| {
            check(copy.owner().is_none())?;
            check(copy.configuration() == Some(current.configuration()))?;
            check(current.check_snapshot(&copy) == Err(EACCES))
        })?;
        Ok(())
    }
}
