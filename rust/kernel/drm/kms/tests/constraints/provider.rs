// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Kernel-controlled CPU provider over private shmem, without descriptors or physical GPUs.
//!
//! Each checked job retains its constraints and prepared mappings. Publication adds source
//! accounting but never replaces the job's backend using a mutable current-selection pointer.

use super::*;
use crate::{drm::preparation::Source, io::Io, types::ForeignOwnable};
use plane::RawPlaneState;

enum Storage {
    Xrgb {
        map: gem::shmem::VMapOwned<TestObject>,
        offset: usize,
    },
    Nv12 {
        luma: gem::shmem::VMapOwned<TestObject>,
        chroma: gem::shmem::VMapOwned<TestObject>,
        y_offset: usize,
        uv_offset: usize,
    },
}

pub(in crate::drm::kms::tests) struct Prepared {
    binding: ARef<OpaqueEntry>,
    storage: Storage,
    // Published exactly once; the owned source reference is released only in Drop.
    source: AtomicPtr<Source>,
}

impl Drop for Prepared {
    fn drop(&mut self) {
        if let Some(source) = NonNull::new(*self.source.get_mut()) {
            // SAFETY: Final job destruction consumes the sole reference installed by activate.
            drop(unsafe { ARef::from_raw(source) });
        }
    }
}

impl Prepared {
    fn activate(&self, source: ARef<Source>) -> Result {
        let raw = ARef::into_raw(source).as_ptr();
        if self
            .source
            .compare_exchange(ptr::null_mut(), raw, Ordering::Release, Ordering::Relaxed)
            .is_err()
        {
            // SAFETY: Failed publication did not consume the incoming reference.
            drop(unsafe { ARef::from_raw(NonNull::new_unchecked(raw)) });
            return Err(EALREADY);
        }
        Ok(())
    }

    fn source(&self) -> Result<&Source> {
        let source = NonNull::new(self.source.load(Ordering::Acquire)).ok_or(EINVAL)?;
        // SAFETY: Activation publishes one owned reference; it cannot be replaced or released
        // before this job's final destruction. The borrow retains the job throughout access.
        Ok(unsafe { source.as_ref() })
    }

    fn sample(&self) -> Result<u32> {
        match &self.storage {
            Storage::Xrgb { map, offset } => {
                Ok(u32::from_le((&*map).try_read32(*offset)?) & 0xffffff)
            }
            Storage::Nv12 {
                luma,
                chroma,
                y_offset,
                uv_offset,
            } => {
                // The test plane uses the fixed BT.601 limited-range default.
                let y = i32::from((&*luma).try_read8(*y_offset)?) - 16;
                let u = i32::from((&*chroma).try_read8(*uv_offset)?) - 128;
                let v = i32::from((&*chroma).try_read8(*uv_offset + 1)?) - 128;
                let component = |value: i32| ((value + 128) >> 8).clamp(0, 255) as u32;
                Ok(component(298 * y + 409 * v) << 16
                    | component(298 * y - 100 * u - 208 * v) << 8
                    | component(298 * y + 516 * u))
            }
        }
    }

    fn render(&self) -> Result<u32> {
        // These sources belong solely to the private test owner; the one-pixel destination is
        // reserved on the stack before admission and no external writer can mutate the buffers.
        let claim = self.source()?.claim()?;
        let result = self.sample();
        claim.release_cpu();
        result
    }
}

/// One owned publication; consumers take ownership instead of racing a borrowed atomic pointer.
#[derive(Default)]
pub(in crate::drm::kms::tests) struct Published(AtomicPtr<Prepared>);

impl Published {
    fn replace(&self, job: Option<Arc<Prepared>>) {
        let raw = job.map_or(ptr::null_mut(), |job| job.into_foreign().cast());
        let old = self.0.swap(raw, Ordering::AcqRel);
        if !old.is_null() {
            // SAFETY: The swap transfers the slot's sole foreign Arc exactly once.
            drop(unsafe { Arc::<Prepared>::from_foreign(old.cast()) });
        }
    }

    fn take(&self) -> Option<Arc<Prepared>> {
        let raw = NonNull::new(self.0.swap(ptr::null_mut(), Ordering::AcqRel))?;
        // SAFETY: The swap transfers the stored foreign Arc to this caller alone.
        Some(unsafe { Arc::from_foreign(raw.as_ptr().cast()) })
    }
}

impl Drop for Published {
    fn drop(&mut self) {
        drop(self.take());
    }
}

pub(in crate::drm::kms::tests) fn prepare(
    transaction: &atomic::AtomicStateComposer<TestDriver>,
    state: &crtc::CrtcStateMutator<'_, crtc::CrtcState<CrtcPayload>>,
) -> Result<Option<Arc<Prepared>>> {
    if state.counts.constraints_render.load(Ordering::Relaxed) == 0 || !state.active() {
        return Ok(None);
    }
    let plane = transaction
        .get_new_plane_state(state.crtc().primary_plane())
        .ok_or(EINVAL)?;
    let image = plane.framebuffer().ok_or(EINVAL)?;
    let binding = state.constraints_entry().ok_or(EINVAL)?;
    if !binding.description().formats().iter().any(|format| {
        format.plane_id() == plane.plane().object_id()
            && format.format() == image.format()
            && format.modifier() == fourcc::FORMAT_MOD_LINEAR
    }) || image
        .modifier()
        .is_some_and(|modifier| modifier != fourcc::FORMAT_MOD_LINEAR)
    {
        return Err(EINVAL);
    }
    // Mapping prepares resources only; pixel reads wait for accepted publication and admission.
    let storage = match image.format() {
        fourcc::XRGB8888 => Storage::Xrgb {
            map: image.object_at(0)?.owned_vmap()?,
            offset: image.offset(0)? as usize,
        },
        fourcc::NV12 => Storage::Nv12 {
            luma: image.object_at(0)?.owned_vmap()?,
            chroma: image.object_at(1)?.owned_vmap()?,
            y_offset: image.offset(0)? as usize,
            uv_offset: image.offset(1)? as usize,
        },
        _ => return Err(EOPNOTSUPP),
    };
    Ok(Some(Arc::new(
        Prepared {
            binding: binding.into(),
            storage,
            source: AtomicPtr::new(ptr::null_mut()),
        },
        GFP_KERNEL,
    )?))
}

pub(in crate::drm::kms::tests) fn verify(
    state: &crtc::CrtcState<CrtcPayload>,
    entry: &OpaqueEntry,
) -> Result {
    if state.counts.constraints_render.load(Ordering::Relaxed) != 0 && state.active() {
        let job = state.work.as_ref().ok_or(EINVAL)?;
        if !ptr::eq(&*job.binding, entry) {
            return Err(EINVAL);
        }
    }
    Ok(())
}

pub(in crate::drm::kms::tests) fn publish(commit: &crtc::CrtcAtomicCommit<'_, TestCrtc>) -> Result {
    let (_, new) = commit.old_new_state();
    if let Some(job) = &new.work {
        job.activate(commit.preparation_source().ok_or(EINVAL)?)?;
    }
    let counts = &commit.crtc().life.0;
    let publication = match commit.crtc().index() {
        0 => &counts.constraints_work,
        1 => &counts.constraints_work_secondary,
        _ => return Err(EINVAL),
    };
    publication.replace(new.work.clone());
    Ok(())
}

fn nv12(
    dev: &testing::TestDevice<TestDriver>,
    counts: &Arc<Counts>,
) -> Result<framebuffer::FramebufferRef<TestDriver>> {
    let object = gem::shmem::Object::<TestObject>::new(
        dev.device(),
        crate::page::page_align(640 * 480 * 3 / 2).ok_or(EOVERFLOW)?,
        Default::default(),
        (),
    )?;
    {
        let map = object.vmap::<0>()?;
        (&map).try_write8(235, 0)?;
        (&map).try_write8(128, 640 * 480)?;
        (&map).try_write8(128, 640 * 480 + 1)?;
    }
    dev.framebuffer(
        &framebuffer::FramebufferLayout {
            width: 640,
            height: 480,
            format: fourcc::NV12,
            modifier: None,
            interlaced: false,
            planes: &[
                framebuffer::FramebufferPlane {
                    object: &object,
                    pitch: 640,
                    offset: 0,
                },
                framebuffer::FramebufferPlane {
                    object: &object,
                    pitch: 640,
                    offset: 640 * 480,
                },
            ],
        },
        framebuffers::Metadata::new(counts, false),
    )
}

#[kunit_tests(rust_drm_constraints_provider)]
mod cases {
    use super::*;
    use crate::{dma_fence::testing::ManualFence, sync::Completion, workqueue};

    // Every exit starts and joins submitted work before the private device/parent can unwind.
    struct InFlight {
        start: Arc<Completion>,
        read_done: Arc<Completion>,
        update_done: Option<Arc<Completion>>,
    }

    impl Drop for InFlight {
        fn drop(&mut self) {
            self.start.complete_all();
            self.read_done.wait_for_completion();
            if let Some(done) = &self.update_done {
                done.wait_for_completion();
            }
        }
    }

    struct ReaderTask(Arc<Completion>);

    impl Drop for ReaderTask {
        fn drop(&mut self) {
            self.0.wait_for_completion();
        }
    }

    struct OtherOutputRead {
        release: Arc<AtomicU32>,
        done: Arc<Completion>,
    }

    impl Drop for OtherOutputRead {
        fn drop(&mut self) {
            self.release.store(1, Ordering::Release);
            self.done.wait_for_completion();
        }
    }

    #[test]
    fn target_animation_does_not_wait_for_another_outputs_read() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        counts.constraints_capacity.store(8, Ordering::Relaxed);
        counts.preparation_capacity.store(8, Ordering::Relaxed);
        counts.constraints_render.store(1, Ordering::Relaxed);
        counts.output_count.store(2, Ordering::Relaxed);
        let parent = faux::Registration::new(c"rust-constraints-two-outputs", None)?;
        let dev = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let first_crtc = dev.crtc_at(0)?;
        let second_crtc = dev.crtc_at(1)?;
        let first_output = dev.constraints_output(0)?;
        let second_output = dev.constraints_output(1)?;
        let first_default = first_output.default_entry().id();
        let second_default = second_output.default_entry().id();
        let target = format_entry(
            first_output.domain(),
            first_crtc.object_id(),
            dev.plane_at(0)?.object_id(),
            fourcc::NV12,
        )?;
        first_output.add(&target)?;
        let image = nv12(&dev, &counts)?;
        let gray = nv12(&dev, &counts)?;
        (&gray.object_at(0)?.vmap::<0>()?).try_write8(81, 0)?;
        let other_image = framebuffer(dev.device())?;
        (&other_image.object_at(0)?.vmap::<0>()?).try_write32(0x00334455u32.to_le(), 0)?;
        let mode = mode()?;
        let mut scanout = atomic::CrtcScanout {
            mode: &mode,
            framebuffer: &image,
            connectors: &[dev.connector_at(0)?],
            position: (0, 0),
        };
        let other_scanout = atomic::CrtcScanout {
            mode: &mode,
            framebuffer: &other_image,
            connectors: &[dev.connector_at(1)?],
            position: (0, 0),
        };
        dev.update(|state| state.set_crtc_config(second_crtc, Some(&other_scanout)))?;
        let other_job = counts.constraints_work_secondary.take().ok_or(EINVAL)?;
        assert_eq!(other_job.render(), Ok(0x334455));
        let other_generation = second_output.snapshot(0)?.info().generation;
        let release = Arc::new(AtomicU32::new(0), GFP_KERNEL)?;
        let worker_release = release.clone();
        let done = Arc::pin_init(Completion::new(), GFP_KERNEL)?;
        let worker_done = done.clone();
        let error = Arc::new(AtomicI32::new(EINPROGRESS.to_errno()), GFP_KERNEL)?;
        let worker_error = error.clone();
        let read = other_job.source()?.claim()?;
        workqueue::system_dfl().try_spawn(GFP_KERNEL, move || {
            for _ in 0..5000 {
                if worker_release.load(Ordering::Acquire) != 0 {
                    break;
                }
                // SAFETY: The test worker holds no locks and bounds a regression's read hold.
                unsafe { bindings::msleep(1) };
            }
            let timed_out = worker_release.load(Ordering::Acquire) == 0;
            read.release_cpu();
            worker_error.store(
                if timed_out { ETIMEDOUT.to_errno() } else { 0 },
                Ordering::Release,
            );
            worker_done.complete_all();
        })?;
        let reader = OtherOutputRead { release, done };
        let result = (|| {
            dev.update(|mut state| {
                state.as_mut().set_crtc_config(first_crtc, Some(&scanout))?;
                state.add_crtc_state(first_crtc)?.set_constraints(&target)
            })?;
            let generation = first_output.snapshot(0)?.info().generation;
            let mut expected = 0xffffff;
            for frame in 0..8 {
                let job = counts.constraints_work.take().ok_or(EINVAL)?;
                if job.binding.id() != target.id() || job.render()? != expected {
                    return Err(EIO);
                }
                scanout.framebuffer = if frame % 2 == 0 { &gray } else { &image };
                expected = if frame % 2 == 0 { 0x4c4c4c } else { 0xffffff };
                dev.update(|state| state.set_crtc_config(first_crtc, Some(&scanout)))?;
                if first_output.snapshot(0)?.info().generation != generation
                    || second_output.snapshot(0)?.info().generation != other_generation
                    || second_output.selected().id() != second_default
                    || counts.constraints_work_secondary.take().is_some()
                    || other_job.render()? != 0x334455
                {
                    return Err(EINVAL);
                }
            }
            if counts.constraints_work.take().ok_or(EINVAL)?.render()? != expected {
                return Err(EIO);
            }
            Ok(())
        })();
        drop(reader);
        assert_eq!(error.load(Ordering::Acquire), 0);
        assert_eq!(result, Ok(()));
        // Full multi-output shutdown retains each binding until default restoration.
        dev.update(|mut state| {
            state.as_mut().set_crtc_config(first_crtc, None)?;
            state.set_crtc_config(second_crtc, None)
        })?;
        assert_eq!(first_output.selected().id(), target.id());
        assert_eq!(second_output.selected().id(), second_default);
        assert!(counts.constraints_work.take().is_none());
        assert!(counts.constraints_work_secondary.take().is_none());
        assert_eq!(first_output.restore_default(), Ok(()));
        assert_eq!(second_output.restore_default(), Ok(()));
        assert_eq!(first_output.selected().id(), first_default);
        first_output.close();
        second_output.close();
        drop(other_job);
        drop(image);
        drop(gray);
        drop(other_image);
        drop(first_output);
        drop(second_output);
        drop(dev);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn waiting_request_rebuild_rejects_a_withdrawn_target() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        counts.constraints_capacity.store(4, Ordering::Relaxed);
        counts.preparation_capacity.store(8, Ordering::Relaxed);
        counts.constraints_render.store(1, Ordering::Relaxed);
        let parent = faux::Registration::new(c"rust-constraints-withdraw-wait", None)?;
        let dev = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let output = dev.constraints_output(0)?;
        let target = format_entry(
            output.domain(),
            dev.crtc()?.object_id(),
            dev.plane()?.object_id(),
            fourcc::NV12,
        )?;
        output.add(&target)?;
        let initial = output.selected().id();
        let first = framebuffer(dev.device())?;
        let second = nv12(&dev, &counts)?;
        let mode = mode()?;
        let mut scanout = atomic::CrtcScanout {
            mode: &mode,
            framebuffer: &first,
            connectors: &[dev.connector()?],
            position: (0, 0),
        };
        dev.update(|state| state.set_crtc_config(dev.crtc()?, Some(&scanout)))?;
        let old = counts.constraints_work.take().ok_or(EINVAL)?;
        let source: ARef<Source> = old.source()?.into();
        let done = Arc::pin_init(Completion::new(), GFP_KERNEL)?;
        let worker_done = done.clone();
        let error = Arc::new(AtomicI32::new(EINPROGRESS.to_errno()), GFP_KERNEL)?;
        let worker_error = error.clone();
        let worker_dev: ARef<Device<TestDriver>> = dev.device().into();
        let target_id = target.id();
        let read = source.claim()?;
        workqueue::system_dfl().try_spawn(GFP_KERNEL, move || {
            let result = (|| {
                use crtc::AsRawCrtc;
                let mut held = false;
                for _ in 0..5000 {
                    match source.claim() {
                        Err(EBUSY) => {
                            held = true;
                            break;
                        }
                        Ok(extra) => extra.release_cpu(),
                        Err(error) => return Err(error),
                    }
                    // SAFETY: No locks are held while awaiting the request's admission hold.
                    unsafe { bindings::msleep(1) };
                }
                if !held {
                    return Err(ETIMEDOUT);
                }
                // SAFETY: ReaderTask joins this worker before private topology or parent teardown.
                let crtc = unsafe {
                    crtc::Crtc::<TestCrtc>::from_raw(worker_dev.crtc.load(Ordering::Relaxed))
                };
                // SAFETY: The private initialized device is retained. The callback only compares
                // accounting identity. Acquiring its modeset lock before releasing the read proves
                // that preparation discarded the attempted state and dropped display locks.
                unsafe {
                    crate::drm::kms::preparation::with_current_source(crtc, |accepted| {
                        if accepted.is_some_and(|accepted| ptr::eq(accepted, &*source)) {
                            Ok(())
                        } else {
                            Err(EINVAL)
                        }
                    })??;
                }
                crate::drm::kms::constraints::Output::<TestDriver>::new(crtc)?.withdraw(target_id)
            })();
            read.release_cpu();
            worker_error.store(result.err().map_or(0, Error::to_errno), Ordering::Release);
            drop(worker_dev);
            worker_done.complete_all();
        })?;
        let reader = ReaderTask(done);
        scanout.framebuffer = &second;
        let mut builds = 0;
        let result = dev.update(|mut state| {
            builds += 1;
            state
                .as_mut()
                .set_crtc_config(dev.crtc()?, Some(&scanout))?;
            state.add_crtc_state(dev.crtc()?)?.set_constraints(&target)
        });
        drop(reader);
        assert_eq!(error.load(Ordering::Acquire), 0);
        assert_eq!(result, Err(ESTALE));
        assert_eq!(builds, 2);
        assert_eq!(output.selected().id(), initial);
        assert!(counts.constraints_work.take().is_none());
        assert!(old.render().is_ok());
        output.close();
        drop(old);
        drop(first);
        drop(second);
        drop(output);
        drop(dev);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn submitted_old_pixels_retire_after_target_acceptance() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        counts.constraints_capacity.store(4, Ordering::Relaxed);
        counts.preparation_capacity.store(8, Ordering::Relaxed);
        counts.constraints_render.store(1, Ordering::Relaxed);
        let parent = faux::Registration::new(c"rust-constraints-pending-pixels", None)?;
        let dev = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let output = dev.constraints_output(0)?;
        let target = format_entry(
            output.domain(),
            dev.crtc()?.object_id(),
            dev.plane()?.object_id(),
            fourcc::NV12,
        )?;
        output.add(&target)?;
        let first = framebuffer(dev.device())?;
        (&first.object_at(0)?.vmap::<0>()?)
            .try_write32(0x00112233u32.to_le(), first.offset(0)? as usize)?;
        let second = nv12(&dev, &counts)?;
        let initial_mode = mode()?;
        let scanout = atomic::CrtcScanout {
            mode: &initial_mode,
            framebuffer: &first,
            connectors: &[dev.connector()?],
            position: (0, 0),
        };
        dev.update(|state| state.set_crtc_config(dev.crtc()?, Some(&scanout)))?;
        let old = counts.constraints_work.take().ok_or(EINVAL)?;
        let start = Arc::pin_init(Completion::new(), GFP_KERNEL)?;
        let read_done = Arc::pin_init(Completion::new(), GFP_KERNEL)?;
        let update_done = Arc::pin_init(Completion::new(), GFP_KERNEL)?;
        let pixel = Arc::new(AtomicU32::new(0), GFP_KERNEL)?;
        let read_error = Arc::new(AtomicI32::new(EINPROGRESS.to_errno()), GFP_KERNEL)?;
        let update_error = Arc::new(AtomicI32::new(EINPROGRESS.to_errno()), GFP_KERNEL)?;
        let mut signal = ManualFence::new()?;
        let fence = signal.fence();
        let read = old.source()?.claim()?;
        let read_job = old.clone();
        let read_start = start.clone();
        let read_finished = read_done.clone();
        let read_pixel = pixel.clone();
        let read_result = read_error.clone();
        if let Err(error) = workqueue::system_dfl().try_spawn(GFP_KERNEL, move || {
            read_start.wait_for_completion();
            // Work was queued before relinquishing the claim. The fence covers every read
            // performed here, and no further work is submitted under that claim.
            let result = read_job.sample();
            if let Ok(value) = result {
                read_pixel.store(value, Ordering::Relaxed);
            }
            read_result.store(result.err().map_or(0, Error::to_errno), Ordering::Release);
            drop(read_job);
            let _ = signal.complete(result.map(|_| ()));
            read_finished.complete_all();
        }) {
            read.release_cpu();
            return Err(error.into());
        }
        let mut flight = InFlight {
            start,
            read_done,
            update_done: None,
        };
        read.release_submitted(&fence);
        let worker_dev: ARef<Device<TestDriver>> = dev.device().into();
        let worker_image = second.clone();
        let worker_target = target.clone();
        let worker_done = update_done.clone();
        let worker_result = update_error.clone();
        workqueue::system_dfl().try_spawn(GFP_KERNEL, move || {
            let result = (|| {
                use connector::AsRawConnector;
                use crtc::AsRawCrtc;
                let mode = mode()?;
                // SAFETY: Setup recorded these objects on the retained private device. InFlight
                // joins this task before fixture teardown or faux-parent deregistration.
                let crtc = unsafe {
                    crtc::Crtc::<TestCrtc>::from_raw(worker_dev.crtc.load(Ordering::Relaxed))
                };
                let connector = unsafe {
                    connector::Connector::<TestConnector>::from_raw(
                        worker_dev.connector.load(Ordering::Relaxed),
                    )
                };
                let scanout = atomic::CrtcScanout {
                    mode: &mode,
                    framebuffer: &worker_image,
                    connectors: &[connector],
                    position: (0, 0),
                };
                // SAFETY: The initialized private topology is retained and no setup, registration
                // or teardown can occur until the main task joins this worker through InFlight.
                unsafe {
                    atomic::run_update(&worker_dev, |mut state| {
                        state.as_mut().set_crtc_config(crtc, Some(&scanout))?;
                        state.add_crtc_state(crtc)?.set_constraints(&worker_target)
                    })
                }
            })();
            worker_result.store(result.err().map_or(0, Error::to_errno), Ordering::Release);
            drop(worker_image);
            drop(worker_dev);
            worker_done.complete_all();
        })?;
        flight.update_done = Some(update_done);
        for _ in 0..5000 {
            if output.selected().id() == target.id()
                || update_error.load(Ordering::Acquire) != EINPROGRESS.to_errno()
            {
                break;
            }
            // SAFETY: The test holds no locks; bounded polling only observes list selection.
            unsafe { bindings::msleep(1) };
        }
        let accepted = output.selected().id() == target.id();
        let old_admission_closed = match old.source()?.claim() {
            Err(EBUSY) => true,
            Ok(extra) => {
                extra.release_cpu();
                false
            }
            Err(_) => false,
        };
        let early_publication = counts.constraints_work.take();
        let awaiting_retirement = update_error.load(Ordering::Acquire) == EINPROGRESS.to_errno();
        let pending = fence.status() == crate::dma_fence::Status::Pending;
        output.close();
        drop(flight);
        assert!(accepted);
        assert!(old_admission_closed);
        assert!(pending);
        assert!(awaiting_retirement);
        assert!(early_publication.is_none());
        let published = counts.constraints_work.take().ok_or(EINVAL)?;
        assert_eq!(published.render(), Ok(0xffffff));
        assert_eq!(pixel.load(Ordering::Relaxed), 0x112233);
        assert_eq!(read_error.load(Ordering::Acquire), 0);
        assert_eq!(update_error.load(Ordering::Acquire), 0);
        assert_ne!(old.binding.id(), published.binding.id());
        drop(published);
        drop(old);
        drop(first);
        drop(second);
        drop(output);
        drop(dev);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn disabled_default_restoration_preserves_retained_job_identity() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        counts.constraints_capacity.store(4, Ordering::Relaxed);
        counts.preparation_capacity.store(8, Ordering::Relaxed);
        counts.constraints_render.store(1, Ordering::Relaxed);
        let parent = faux::Registration::new(c"rust-constraints-default-restore", None)?;
        let dev = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let output = dev.constraints_output(0)?;
        let initial = output.default_entry().id();
        let target = format_entry(
            output.domain(),
            dev.crtc()?.object_id(),
            dev.plane()?.object_id(),
            fourcc::NV12,
        )?;
        output.add(&target)?;
        let image = nv12(&dev, &counts)?;
        let mode = mode()?;
        let scanout = atomic::CrtcScanout {
            mode: &mode,
            framebuffer: &image,
            connectors: &[dev.connector()?],
            position: (0, 0),
        };
        dev.update(|mut state| {
            state
                .as_mut()
                .set_crtc_config(dev.crtc()?, Some(&scanout))?;
            state.add_crtc_state(dev.crtc()?)?.set_constraints(&target)
        })?;
        let retained = counts.constraints_work.take().ok_or(EINVAL)?;
        assert_eq!(retained.render(), Ok(0xffffff));
        assert_eq!(output.restore_default(), Err(EBUSY));
        assert_eq!(output.selected().id(), target.id());

        // The private owner admits no concurrent requests or external source consumers.
        // Disable and retire its old scanout before requesting the fixed default.
        dev.update(|state| state.set_crtc_config(dev.crtc()?, None))?;
        assert!(counts.constraints_work.take().is_none());
        assert_eq!(output.selected().id(), target.id());
        counts.fail_constraints.store(1, Ordering::Relaxed);
        assert_eq!(output.restore_default(), Err(EIO));
        assert_eq!(output.selected().id(), target.id());
        counts.fail_constraints.store(0, Ordering::Relaxed);
        assert_eq!(output.restore_default(), Ok(()));
        assert_eq!(output.selected().id(), initial);
        assert_eq!(retained.binding.id(), target.id());
        assert!(counts.constraints_work.take().is_none());
        let generation = output.snapshot(0)?.info().generation;
        assert_eq!(output.restore_default(), Ok(()));
        assert_eq!(output.snapshot(0)?.info().generation, generation);
        output.close();
        assert_eq!(output.restore_default(), Err(ESTALE));
        assert_eq!(output.add(&target), Err(ESTALE));
        drop(retained);
        drop(image);
        drop(output);
        drop(dev);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn cpu_jobs_use_the_backend_retained_by_the_accepted_scene() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        counts.constraints_capacity.store(4, Ordering::Relaxed);
        counts.preparation_capacity.store(8, Ordering::Relaxed);
        counts.constraints_render.store(1, Ordering::Relaxed);
        let parent = faux::Registration::new(c"rust-constraints-cpu-provider", None)?;
        let dev = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let output = dev.constraints_output(0)?;
        let target = format_entry(
            output.domain(),
            dev.crtc()?.object_id(),
            dev.plane()?.object_id(),
            fourcc::NV12,
        )?;
        output.add(&target)?;
        let first = framebuffer(dev.device())?;
        (&first.object_at(0)?.vmap::<0>()?)
            .try_write32(0x00112233u32.to_le(), first.offset(0)? as usize)?;
        let second = nv12(&dev, &counts)?;
        let mode = mode()?;
        let connectors = [dev.connector()?];
        let first_scanout = atomic::CrtcScanout {
            mode: &mode,
            framebuffer: &first,
            connectors: &connectors,
            position: (0, 0),
        };
        let second_scanout = atomic::CrtcScanout {
            framebuffer: &second,
            ..first_scanout
        };
        dev.update(|state| state.set_crtc_config(dev.crtc()?, Some(&first_scanout)))?;
        assert_eq!(counts.constraints_work_error.load(Ordering::Relaxed), 0);
        let old = counts.constraints_work.take().ok_or(EINVAL)?;
        assert_eq!(old.binding.id(), output.default_entry().id());
        assert_eq!(old.render()?, 0x112233);
        let select = |mut state: Pin<&mut atomic::AtomicStateComposer<TestDriver>>| {
            state
                .as_mut()
                .set_crtc_config(dev.crtc()?, Some(&second_scanout))?;
            state.add_crtc_state(dev.crtc()?)?.set_constraints(&target)
        };
        dev.check(select)?;
        assert!(counts.constraints_work.take().is_none());
        assert_eq!(output.selected().id(), old.binding.id());
        assert_eq!(old.render()?, 0x112233);
        dev.update(select)?;
        assert_eq!(counts.constraints_work_error.load(Ordering::Relaxed), 0);
        let new = counts.constraints_work.take().ok_or(EINVAL)?;
        assert_eq!(new.binding.id(), target.id());
        assert_eq!(new.render()?, 0xffffff);
        assert_ne!(old.binding.id(), new.binding.id());
        assert!(matches!(old.storage, Storage::Xrgb { .. }));
        assert!(matches!(new.storage, Storage::Nv12 { .. }));
        output.close();
        drop(output);
        drop(first);
        drop(second);
        drop(old);
        drop(new);
        drop(dev);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }
}
