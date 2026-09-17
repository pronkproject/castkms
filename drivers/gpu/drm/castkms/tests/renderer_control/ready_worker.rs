// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::{
    execution::validation::SceneView,
    renderer::{
        private_pool::Pool,
        ready::Worker, //
    }, //
};
use core::sync::atomic::{
    AtomicBool,
    Ordering, //
};
use kernel::{
    dma_fence::testing::ManualFence,
    drm::gem::ExportAccess,
    sync::Completion,
    workqueue::{
        self,
        impl_has_work,
        new_work,
        Work,
        WorkItem, //
    }, //
};

#[pin_data]
struct Revoker {
    worker: Arc<Worker>,
    #[pin]
    work: Work<Self>,
    #[pin]
    entered: Completion,
    finished: AtomicBool,
}

impl_has_work! {
    impl HasWork<Self> for Revoker { self.work }
}

impl WorkItem for Revoker {
    type Pointer = Arc<Self>;

    fn run(revoker: Arc<Self>) {
        revoker.entered.complete_all();
        revoker.worker.revoke();
        revoker.finished.store(true, Ordering::Release);
    }
}

#[kunit_tests(rust_castkms_ready_worker)]
mod cases {
    use super::*;

    #[test]
    fn preparation_requires_registered_storage_and_completed_probe() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let profile = private_images::profile()?;
            let source_geometry = profile.limits().geometry.source;
            let proposal = candidate.propose_profile(profile)?;
            let mut pool = Pool::new()?;
            check(proposal.prepare_worker(&pool, [640, 480]).err() == Some(ENODATA))?;
            let image = proposal.register_image(
                [640, 480],
                &[private_images::buffer(device, ExportAccess::ReadWrite)?],
            )?;
            pool.insert(1, || Ok(image.clone()))?;
            check(proposal.prepare_worker(&pool, [640, 480]).err() == Some(ENODATA))?;
            drop(pool.remove(1)?);
            pool.insert(2, || Ok(image.clone()))?;
            let mut completion = ManualFence::new()?;
            candidate.submit_private_probe(Some(completion.fence()))?;
            check(proposal.prepare_worker(&pool, [640, 480]).err() == Some(EAGAIN))?;
            drop(pool.remove(2)?);
            pool.insert(3, || Ok(image.clone()))?;
            completion.complete(Ok(()))?;
            let ready = proposal.prepare_worker(&pool, [640, 480])?;
            let worker = ready.worker();
            check(worker.profile().limits().geometry.min_output == [640, 480])?;
            check(worker.profile().limits().geometry.output == [640, 480])?;
            check(worker.profile().limits().geometry.source == source_geometry)?;
            let _source = worker.probe_source();
            let guard = worker.hold_ready()?;
            check(guard.contains(3, &image) && !guard.contains(2, &image))?;
            check(worker.check_scene(SceneView::Disabled).is_ok())?;
            let blank = crate::scene::Scene::blank(None);
            check(
                worker
                    .check_scene(SceneView::Enabled {
                        scene: &blank,
                        output: [640, 480],
                    })
                    .is_ok(),
            )?;
            check(
                worker.check_scene(SceneView::Enabled {
                    scene: &blank,
                    output: [800, 600],
                }) == Err(EOPNOTSUPP),
            )?;
            drop(guard);
            check(pool.remove(3).err() == Some(EBUSY))?;
            drop(ready);
            check(worker.check_scene(SceneView::Disabled) == Err(EKEYREVOKED))?;
            check(
                worker
                    .hold_ready()
                    .err()
                    .is_some_and(|error| error == EKEYREVOKED),
            )?;
            drop(pool.remove(3)?);
            Ok(())
        })
    }

    #[test]
    fn failed_probe_does_not_pin_pool_names() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let proposal = candidate.propose_profile(private_images::profile()?)?;
            let mut pool = Pool::new()?;
            pool.insert(1, || {
                proposal.register_image(
                    [640, 480],
                    &[private_images::buffer(device, ExportAccess::ReadWrite)?],
                )
            })?;
            let mut completion = ManualFence::new()?;
            candidate.submit_private_probe(Some(completion.fence()))?;
            completion.complete(Err(EIO))?;
            check(proposal.prepare_worker(&pool, [640, 480]).err() == Some(EIO))?;
            drop(pool.remove(1)?);
            Ok(())
        })
    }

    #[test]
    fn revoked_authority_cannot_prepare_a_worker() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let proposal = candidate.propose_profile(private_images::profile()?)?;
            let mut pool = Pool::new()?;
            pool.insert(1, || {
                proposal.register_image(
                    [640, 480],
                    &[private_images::buffer(device, ExportAccess::ReadWrite)?],
                )
            })?;
            candidate.submit_private_probe(None)?;
            owner.revoke();
            check(
                proposal.prepare_worker(&pool, [640, 480]).err() == Some(EKEYREVOKED),
            )?;
            drop(pool.remove(1)?);
            Ok(())
        })
    }

    #[test]
    fn proposal_geometry_limits_worker_resources() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let profile = private_images::profile()?;
            let mut limits = *profile.limits();
            limits.geometry.min_output = [1, 1];
            limits.geometry.output = [320, 240];
            let mut formats = KVec::new();
            formats.extend_from_slice(profile.formats(), GFP_KERNEL)?;
            let narrow = crate::execution::capabilities::Profile::new(limits, formats)?;
            let proposal = candidate.propose_profile(narrow)?;
            let pool = Pool::new()?;
            candidate.submit_private_probe(None)?;
            let buffer = private_images::buffer(device, ExportAccess::ReadWrite)?;
            check(proposal.register_image([640, 480], &[buffer]).err() == Some(EOPNOTSUPP))?;
            check(proposal.prepare_worker(&pool, [640, 480]).err() == Some(ENODATA))?;
            Ok(())
        })
    }

    #[test]
    fn installation_guard_excludes_revocation_and_releases_resources() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let proposal = candidate.propose_profile(private_images::profile()?)?;
            let mut pool = Pool::new()?;
            pool.insert(1, || {
                proposal.register_image(
                    [640, 480],
                    &[private_images::buffer(device, ExportAccess::ReadWrite)?],
                )
            })?;
            candidate.submit_private_probe(None)?;
            let ready = proposal.prepare_worker(&pool, [640, 480])?;
            let worker = ready.worker();
            let revoking = worker.clone();
            let revoker = Arc::pin_init(
                pin_init!(Revoker {
                    worker: revoking,
                    work <- new_work!("castkms-ready-revoke-test"),
                    entered <- Completion::new(),
                    finished: AtomicBool::new(false),
                }),
                GFP_KERNEL,
            )?;
            let guard = worker.hold_ready()?;
            let queued = workqueue::system_dfl().enqueue(revoker.clone()).is_ok();
            if queued {
                revoker.entered.wait_for_completion();
            }
            let finished = revoker.finished.load(Ordering::Acquire);
            // Native validation can run under the installation guard without relocking it.
            let valid = worker.check_scene(SceneView::Disabled);
            drop(guard);
            revoker.work.flush();
            check(queued && !finished && valid.is_ok())?;
            check(revoker.finished.load(Ordering::Acquire))?;
            drop(pool.remove(1)?);
            drop(ready);
            check(
                worker
                    .hold_ready()
                    .err()
                    .is_some_and(|error| error == EKEYREVOKED),
            )
        })
    }
}
