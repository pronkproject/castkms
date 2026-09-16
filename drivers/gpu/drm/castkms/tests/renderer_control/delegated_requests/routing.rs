// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::renderer::routing::{Prepared, Registry};

fn registry(fixture: &Fixture) -> Result<Arc<Registry>> {
    let access = fixture.owner.access();
    Registry::new(access.display().output.identity(), access.device().changed.clone())
}

#[kunit_tests(rust_castkms_renderer_routing)]
mod cases {
    use super::*;

    #[test]
    fn disabled_video_does_not_prevent_worker_metadata_publication() -> Result {
        with_display(|device, crtc, connector, scanout, file| {
            let fixture = output_fixture(device, crtc, connector, &file)?;
            let registry = registry(&fixture)?;
            device.atomic_update(|transaction| transaction.set_crtc_config(crtc, None))?;
            let _owner = registry.publish(Prepared::new()?, &fixture.renderer, &fixture.active)?;
            check(registry.lookup().err() == Some(ENODEV))?;
            fixture.active.check()?;
            device.atomic_update(|transaction| {
                transaction.set_crtc_config(crtc, Some(scanout))
            })?;
            let scope = fixture.grantor.capture().describe_delegated()?;
            drop(registry.lookup()?.create_queue(&scope, 1)?);
            Ok(())
        })
    }

    #[test]
    fn discovered_routes_do_not_extend_worker_ownership() -> Result {
        with_output(|fixture| {
            let registry = registry(&fixture)?;
            check(registry.lookup().err() == Some(ENODEV))?;
            let owner = registry.publish(Prepared::new()?, &fixture.renderer, &fixture.active)?;
            let route = registry.lookup()?;
            let scope = fixture.grantor.capture().describe_delegated()?;
            let mut queue = route.create_queue(&scope, 1)?;
            queue.queue_to(1, &fixture.destination, None)?;
            drop(fixture.active);
            check(registry.lookup().err() == Some(EIO))?;
            check(route.create_queue(&scope, 1).err() == Some(EIO))?;
            check(queue.advance() == 1)?;
            queue.dequeue(|result| check(result.result == Err(EIO)))?;
            drop(owner);
            check(registry.lookup().err() == Some(ENODEV))
        })
    }

    #[test]
    fn closed_directory_cannot_be_republished() -> Result {
        with_output(|fixture| {
            let registry = registry(&fixture)?;
            let owner = registry.publish(Prepared::new()?, &fixture.renderer, &fixture.active)?;
            registry.close();
            check(registry.lookup().err() == Some(ESHUTDOWN))?;
            check(
                registry.publish(Prepared::new()?, &fixture.renderer, &fixture.active).err()
                    == Some(ESHUTDOWN),
            )?;
            drop(owner);
            registry.close();
            check(registry.lookup().err() == Some(ESHUTDOWN))
        })
    }

    #[test]
    fn routes_cannot_be_published_for_another_output() -> Result {
        with_output(|fixture| {
            let output = Arc::pin_init(crate::Output::new(), GFP_KERNEL)?;
            let registry = Registry::new(
                output.identity(),
                fixture.owner.access().device().changed.clone(),
            )?;
            check(
                registry.publish(Prepared::new()?, &fixture.renderer, &fixture.active).err()
                    == Some(EINVAL),
            )?;
            check(registry.lookup().err() == Some(ENODEV))
        })
    }

    #[test]
    fn replaced_owner_cannot_remove_or_republish_over_its_successor() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let fixture = output_fixture(device, crtc, connector, &file)?;
            let registry = registry(&fixture)?;
            let old = registry.publish(Prepared::new()?, &fixture.renderer, &fixture.active)?;
            let renderer = Arc::new(Candidate::begin(fixture.owner.access())?, GFP_KERNEL)?;
            let (active, _) = activate(&renderer, device, crtc)?;
            let new = registry.publish(Prepared::new()?, &renderer, &active)?;
            let route = registry.lookup()?;
            check(
                registry.publish(Prepared::new()?, &fixture.renderer, &fixture.active).err()
                    == Some(EIO),
            )?;
            drop(old);
            check(Arc::ptr_eq(&route, &registry.lookup()?))?;
            let scope = fixture.grantor.capture().describe_delegated()?;
            drop(route.create_queue(&scope, 1)?);
            drop(new);
            check(registry.lookup().err() == Some(ENODEV))
        })
    }
}
