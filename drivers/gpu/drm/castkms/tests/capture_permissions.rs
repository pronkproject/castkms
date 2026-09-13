// SPDX-License-Identifier: GPL-2.0-only

//! Native master control and accepted ownership, without a userspace capture grant.

use super::*;
use crate::{
    capture::permission::Permission,
    host_compositor::{
        compose,
        layout::Layout,
        pool::Pool, //
    }, //
};
use kernel::drm::kms::testing::MasterFile;

fn permission(fixture: &Fixture, file: &MasterFile<'_, Driver>) -> Result<Permission> {
    let snapshot = file.file().master_snapshot().ok_or(EINVAL)?;
    let guard = snapshot.master().lock_current().ok_or(EACCES)?;
    Permission::new(&guard, fixture.drm.crtc()?, fixture.drm.connector()?)
}

fn pool(fixture: &Fixture) -> Result<kernel::sync::Arc<Pool>> {
    Pool::new(
        fixture.drm.device(),
        &fixture.host_budget,
        Layout::new(640, 480)?,
    )
}

#[kunit_tests(rust_castkms_capture_permissions)]
mod cases {
    use super::*;

    #[test]
    fn current_owner_accepts_its_independently_completed_image() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let permission = permission(&fixture, &file)?;
        check(matches!(permission.with_current(|_| Ok(())), Err(ENODEV)))?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(
            file.file().master_snapshot(),
        ))?;
        fixture.select(&fb, false, 0)?;
        let image =
            compose::current(&fixture.drm.device().output, &pool(&fixture)?)?.ok_or(EINVAL)?;
        let configuration = permission.with_current(|current| {
            check(current.layout() == image.layout())?;
            current.check_image(&image)?;
            Ok(current.configuration().clone())
        })?;
        check(image.configuration() == Some(&configuration))?;
        fixture.select(&fb, false, 0)?;
        permission.with_current(|current| current.check_image(&image))?;
        Ok(())
    }

    #[test]
    fn retained_identity_does_not_preserve_current_control() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let permission = permission(&fixture, &file)?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(
            file.file().master_snapshot(),
        ))?;
        fixture.select(&fb, false, 0)?;
        permission.with_current(|_| Ok(()))?;
        drop(file);
        check(matches!(permission.with_current(|_| Ok(())), Err(EACCES)))?;
        let replacement = fixture.drm.master_file()?;
        check(matches!(permission.with_current(|_| Ok(())), Err(EACCES)))?;
        let next = super::permission(&fixture, &replacement)?;
        check(matches!(next.with_current(|_| Ok(())), Err(EACCES)))?;
        Ok(())
    }

    #[test]
    fn object_membership_is_rechecked_after_connector_removal() -> Result {
        let fixture = Fixture::new()?;
        let file = fixture.drm.master_file()?;
        check(matches!(permission(&fixture, &file), Err(EACCES)))?;
        let connector = fixture.drm.publish_connector_identity()?;
        let permission = permission(&fixture, &file)?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(
            file.file().master_snapshot(),
        ))?;
        fixture.select(&fb, false, 0)?;
        permission.with_current(|_| Ok(()))?;
        drop(connector);
        check(matches!(permission.with_current(|_| Ok(())), Err(EACCES)))?;
        Ok(())
    }

    #[test]
    fn a_foreign_device_is_not_an_issuable_target() -> Result {
        let fixture = Fixture::new()?;
        let other = Fixture::new_named(c"castkms-permission-other")?;
        let _first_connector = fixture.drm.publish_connector_identity()?;
        let _other_connector = other.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let snapshot = file.file().master_snapshot().ok_or(EINVAL)?;
        let guard = snapshot.master().lock_current().ok_or(EINVAL)?;
        check(matches!(
            Permission::new(&guard, other.drm.crtc()?, fixture.drm.connector()?),
            Err(EACCES)
        ))?;
        check(matches!(
            Permission::new(&guard, fixture.drm.crtc()?, other.drm.connector()?),
            Err(EACCES)
        ))?;
        Ok(())
    }

    #[test]
    fn matching_layout_does_not_substitute_for_the_image_output() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let permission = permission(&fixture, &file)?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(
            file.file().master_snapshot(),
        ))?;
        fixture.select(&fb, false, 0)?;
        let output = &fixture.drm.device().output;
        let configuration =
            permission.with_current(|current| Ok(current.configuration().clone()))?;
        let other = kernel::sync::Arc::pin_init(Output::new(), GFP_KERNEL)?;
        other.publish_with_configuration(
            kernel::drm::preparation::Source::new(1)?,
            output::SceneUpdate::Replace(output.inspect(|scene| scene.cloned())),
            Some(configuration),
        );
        let image = compose::current(&other, &pool(&fixture)?)?.ok_or(EINVAL)?;
        check(matches!(
            permission.with_current(|current| current.check_image(&image)),
            Err(EACCES)
        ))?;
        other.close();
        Ok(())
    }

    #[test]
    fn old_configuration_pixels_fail_after_disable_and_reenable() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let permission = permission(&fixture, &file)?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(
            file.file().master_snapshot(),
        ))?;
        fixture.select(&fb, false, 0)?;
        let image =
            compose::current(&fixture.drm.device().output, &pool(&fixture)?)?.ok_or(EINVAL)?;
        fixture.drm.update(|mut transaction| {
            transaction
                .as_mut()
                .set_crtc_config(fixture.drm.crtc()?, None)
        })?;
        check(matches!(permission.with_current(|_| Ok(())), Err(ENODEV)))?;
        fixture.select(&fb, false, 0)?;
        check(matches!(
            permission.with_current(|current| current.check_image(&image)),
            Err(EACCES)
        ))?;
        Ok(())
    }

    #[test]
    fn historical_pixels_need_the_recipient_owner_even_after_new_content() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let old_file = fixture.drm.master_file()?;
        let old_fb = fixture.framebuffer(provenance::Provenance::from_snapshot(
            old_file.file().master_snapshot(),
        ))?;
        fixture.select(&old_fb, false, 0)?;
        let images = pool(&fixture)?;
        let old_image = compose::current(&fixture.drm.device().output, &images)?.ok_or(EINVAL)?;
        drop(old_file);
        let new_file = fixture.drm.master_file()?;
        let permission = permission(&fixture, &new_file)?;
        let new_fb = fixture.framebuffer(provenance::Provenance::from_snapshot(
            new_file.file().master_snapshot(),
        ))?;
        fixture.select(&new_fb, false, 0)?;
        let new_image = compose::current(&fixture.drm.device().output, &images)?.ok_or(EINVAL)?;
        check(old_image.configuration() == new_image.configuration())?;
        permission.with_current(|current| current.check_image(&new_image))?;
        check(matches!(
            permission.with_current(|current| current.check_image(&old_image)),
            Err(EACCES)
        ))?;
        Ok(())
    }

    #[test]
    fn unowned_pixels_do_not_become_authorized_on_master_acquisition() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let file = fixture.drm.master_file()?;
        let permission = permission(&fixture, &file)?;
        check(matches!(permission.with_current(|_| Ok(())), Err(EACCES)))?;
        fixture.select(&fb, false, 0)?;
        check(matches!(permission.with_current(|_| Ok(())), Err(EACCES)))?;
        Ok(())
    }
}
