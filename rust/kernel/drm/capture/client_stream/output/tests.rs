// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::{
    dma_fence::{
        testing::ManualFence,
        Status, //
    },
    drm::capture::{
        Authority,
        ClientOwner,
        Completion,
        Policy,
        Readiness, //
    },
    fs::File,
    sync::{
        aref::ARef,
        Arc, //
    },
    time::{
        Instant,
        Monotonic, //
    }, //
};

struct TestPolicy;

// SAFETY: Policy callbacks and destruction belong to LocalModule.
#[vtable]
unsafe impl Policy for TestPolicy {
    fn revoke(&self) {}
}

struct Owner {
    opened: bool,
    last_use: u64,
    pending: Option<(u64, Option<ARef<Fence>>)>,
    readiness: ARef<Readiness>,
}

// SAFETY: All owner callbacks and destruction belong to LocalModule.
#[vtable]
unsafe impl ClientOwner for Owner {
    fn open_stream(&mut self, id: u64, offer: u64, capacity: u32) -> Result {
        if id != 7 || offer != 9 || capacity != 1 || self.opened {
            return Err(EINVAL);
        }
        self.opened = true;
        Ok(())
    }

    fn close_stream(&mut self, id: u64) -> Result {
        if id != 7 || !self.opened {
            return Err(ENOENT);
        }
        self.pending = None;
        self.opened = false;
        self.readiness.update(false);
        Ok(())
    }

    fn readiness(&self) -> Option<&Readiness> {
        Some(&self.readiness)
    }

    fn queue_output(
        &mut self,
        stream: u64,
        use_id: u64,
        destination: u64,
        reuse: Option<&Fence>,
    ) -> Result {
        if stream != 7 || !self.opened || destination != 23 {
            return Err(ENOENT);
        }
        if use_id <= self.last_use {
            return Err(EINVAL);
        }
        if self.pending.is_some() {
            return Err(EBUSY);
        }
        self.pending = Some((use_id, reuse.map(Fence::to_owned_ref)));
        self.last_use = use_id;
        Ok(())
    }

    fn dequeue(&mut self, stream: u64, publish: impl FnOnce(Completion) -> Result) -> Result {
        if stream != 7 || !self.opened {
            return Err(ENOENT);
        }
        let (use_id, reuse) = self.pending.as_ref().ok_or(EAGAIN)?;
        let result = match reuse.as_ref().map(|fence| fence.status()) {
            Some(Status::Pending) => return Err(EAGAIN),
            Some(Status::Complete(Err(error))) => Err(error),
            _ => Ok(Instant::<Monotonic>::now()),
        };
        publish(Completion::new(*use_id, result)?)?;
        self.pending = None;
        Ok(())
    }
}

fn client() -> Result<(ARef<Authority<TestPolicy>>, ARef<File>, ClientStream)> {
    let authority = Authority::new(Arc::new(TestPolicy, GFP_KERNEL)?)?;
    let file = authority.create_client_file(Owner {
        opened: false,
        last_use: 0,
        pending: None,
        readiness: Readiness::new()?,
    })?;
    let stream = ClientStream::open(&file, 7, 9, 1)?;
    Ok((authority, file, stream))
}

fn release(file: ARef<File>) {
    // SAFETY: The test kernel thread transfers its final owned file reference.
    unsafe { bindings::__fput_sync(ARef::into_raw(file).cast().as_ptr()) };
}

#[kunit_tests(rust_drm_capture_client_output)]
mod cases {
    use super::*;

    #[test]
    fn output_retains_the_borrowed_reuse_fence() -> Result {
        let (_authority, file, stream) = client()?;
        let reuse = ManualFence::new()?;
        stream.queue_output(1, 23, Some(&reuse.fence()))?;
        assert_eq!(stream.dequeue(|_| Ok(())), Err(EAGAIN));
        drop(reuse);
        let completion = stream.dequeue(Ok)?;
        assert_eq!(completion.use_id(), 1);
        assert!(matches!(completion.result(), Err(EIO)));
        drop(stream);
        release(file);
        Ok(())
    }

    #[test]
    fn rejected_output_preserves_its_name_for_retry() -> Result {
        let (_authority, file, stream) = client()?;
        assert_eq!(stream.queue_output(0, 23, None), Err(EINVAL));
        assert_eq!(stream.queue_output(1, 0, None), Err(EINVAL));
        assert_eq!(stream.queue_output(1, 24, None), Err(ENOENT));
        stream.queue_output(1, 23, None)?;
        assert_eq!(stream.queue_output(2, 23, None), Err(EBUSY));
        assert_eq!(stream.dequeue(|_| Err::<(), _>(EFAULT)), Err(EFAULT));
        assert_eq!(stream.dequeue(Ok)?.use_id(), 1);
        stream.queue_output(2, 23, None)?;
        assert_eq!(stream.dequeue(Ok)?.use_id(), 2);
        assert_eq!(stream.queue_output(2, 23, None), Err(EINVAL));
        drop(stream);
        release(file);
        Ok(())
    }

    #[test]
    fn revoked_and_closed_streams_reject_new_output() -> Result {
        let (authority, file, mut stream) = client()?;
        stream.queue_output(1, 23, None)?;
        authority.revoke();
        // Native admission rejects revoked authority before asking the provider.
        assert_eq!(stream.queue_output(2, 23, None), Err(EKEYREVOKED));
        assert_eq!(stream.dequeue(Ok)?.use_id(), 1);
        stream.close()?;
        assert_eq!(stream.queue_output(3, 23, None), Err(ENOENT));
        drop(stream);
        release(file);
        Ok(())
    }
}
