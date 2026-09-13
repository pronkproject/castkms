// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::{
    host_compositor::{
        compose,
        layout::Layout,
        pool::Pool, //
    },
    output::SceneUpdate,
    Output, //
};
use kernel::{
    drm::preparation::Source,
    sync::Arc, //
};

#[kunit_tests(rust_castkms_host_origins)]
mod cases {
    use super::*;

    #[test]
    fn identical_scenes_on_different_outputs_keep_distinct_image_origins() -> Result {
        let fixture = Fixture::new()?;
        let owner = fixture.drm.synthetic_master_snapshot(true)?;
        let master = owner.master().clone();
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(Some(owner)))?;
        fixture.select(&fb, false, 0)?;
        let first = &fixture.drm.device().output;
        let scene = first.inspect(|scene| scene.cloned()).ok_or(EINVAL)?;
        let configuration = first
            .with_accepted(|accepted| accepted.and_then(|accepted| accepted.configuration.clone()))
            .ok_or(EINVAL)?;
        let second = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        second.publish_with_configuration(
            Source::new(1)?,
            SceneUpdate::Replace(Some(scene)),
            Some(configuration.clone()),
        );
        let pool = Pool::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(640, 480)?,
        )?;
        let a = compose::current(first, &pool)?.ok_or(EINVAL)?;
        let b = compose::current(&second, &pool)?.ok_or(EINVAL)?;
        check(a.layout() == b.layout())?;
        check(a.content_serial() == b.content_serial())?;
        check(a.owner() == b.owner())?;
        check(a.owner() == Some(&master))?;
        check(a.configuration() == Some(&configuration))?;
        check(b.configuration() == Some(&configuration))?;
        check(a.output_identity() == first.identity())?;
        check(b.output_identity() == second.identity())?;
        check(a.output_identity() != b.output_identity())?;
        second.close();
        fixture.state.close();
        check(a.output_identity() == first.identity())?;
        check(b.output_identity() == second.identity())?;
        Ok(())
    }
}
