// SPDX-License-Identifier: GPL-2.0-only

//! Accepted output generations observed through real CastKMS atomic callbacks.

use super::*;
use kernel::{
    drm::preparation::Source,
    sync::aref::ARef, //
};

fn published(fixture: &Fixture) -> Result<(ARef<Source>, Option<scene::ContentSerial>)> {
    fixture.drm.device().output.inspect_accepted(|current| {
        current
            .map(|(source, scene)| (source.into(), scene.map(scene::Scene::content_serial)))
            .ok_or(EINVAL)
    })
}

#[kunit_tests(rust_castkms_generations)]
mod cases {
    use super::*;

    #[test]
    fn crtc_only_update_preserves_the_plane_description() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let (first, content) = published(&fixture)?;
        assert!(content.is_some());
        fixture.drm.update(|transaction| {
            drop(transaction.add_crtc_state(fixture.drm.crtc()?)?);
            if transaction
                .get_new_plane_state(fixture.drm.plane()?)
                .is_some()
            {
                return Err(EINVAL);
            }
            Ok(())
        })?;
        let (second, retained) = published(&fixture)?;
        assert!(!core::ptr::eq(&*first, &*second));
        assert_eq!(content, retained);
        assert!(matches!(first.claim(), Err(EBUSY)));
        second.claim()?.release_cpu();
        Ok(())
    }

    #[test]
    fn same_framebuffer_commit_publishes_new_content() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let (first, content) = published(&fixture)?;
        fixture.select(&fb, false, 0)?;
        let (second, changed) = published(&fixture)?;
        assert!(!core::ptr::eq(&*first, &*second));
        assert!(changed.is_some() && changed != content);
        assert!(matches!(first.claim(), Err(EBUSY)));
        Ok(())
    }

    #[test]
    fn disable_publishes_a_blank_generation() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let (first, _) = published(&fixture)?;
        fixture.drm.update(|mut transaction| {
            transaction
                .as_mut()
                .set_crtc_config(fixture.drm.crtc()?, None)
        })?;
        let (blank, content) = published(&fixture)?;
        assert!(!core::ptr::eq(&*first, &*blank));
        assert!(content.is_none());
        assert!(matches!(first.claim(), Err(EBUSY)));
        Ok(())
    }

    #[test]
    fn check_only_does_not_replace_the_published_generation() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let (first, content) = published(&fixture)?;
        fixture.select(&fb, true, 0)?;
        let (retained, retained_content) = published(&fixture)?;
        assert!(core::ptr::eq(&*first, &*retained));
        assert_eq!(content, retained_content);
        retained.claim()?.release_cpu();
        Ok(())
    }
}
