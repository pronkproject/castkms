// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::{
    drm::capture::{
        Authority,
        ClientOwner,
        Policy, //
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
use core::cell::Cell;

struct TestPolicy;

// SAFETY: Policy callbacks and destruction belong to LocalModule.
#[vtable]
unsafe impl Policy for TestPolicy {
    fn revoke(&self) {}
}

struct Owner {
    opened: bool,
    completion: Option<Completion>,
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
        self.opened = false;
        Ok(())
    }

    fn dequeue(&mut self, stream: u64, publish: impl FnOnce(Completion) -> Result) -> Result {
        if stream != 7 || !self.opened {
            return Err(ENOENT);
        }
        publish(self.completion.ok_or(EAGAIN)?)?;
        self.completion = None;
        Ok(())
    }
}

fn client(
    completion: Completion,
) -> Result<(ARef<Authority<TestPolicy>>, ARef<File>, ClientStream)> {
    let authority = Authority::new(Arc::new(TestPolicy, GFP_KERNEL)?)?;
    let file = authority.create_client_file(Owner {
        opened: false,
        completion: Some(completion),
    })?;
    let stream = ClientStream::open(&file, 7, 9, 1)?;
    Ok((authority, file, stream))
}

fn release(file: ARef<File>) {
    // SAFETY: The test runs in a kernel thread and transfers its final owned file reference.
    unsafe { bindings::__fput_sync(ARef::into_raw(file).cast().as_ptr()) };
}

#[kunit_tests(rust_drm_capture_client_completion)]
mod cases {
    use super::*;

    #[test]
    fn a_borrowed_publisher_retries_the_same_result_after_revocation() -> Result {
        let time = Instant::<Monotonic>::now();
        let (authority, file, stream) = client(Completion::new(19, Ok(time))?)?;
        let calls = Cell::new(0);
        let result = stream.dequeue::<()>(|completion| {
            calls.set(calls.get() + 1);
            assert_eq!(completion.use_id(), 19);
            Err(EFAULT)
        });
        assert_eq!(result, Err(EFAULT));
        assert_eq!(calls.get(), 1);
        authority.revoke();
        let retried = stream.dequeue(Ok)?;
        assert_eq!(retried.use_id(), 19);
        assert_eq!(retried.result()?.as_nanos(), time.as_nanos());
        assert_eq!(stream.dequeue(|_| Ok(())), Err(EAGAIN));
        drop(stream);
        release(file);
        Ok(())
    }

    #[test]
    fn a_once_only_publisher_transfers_its_owned_return_value() -> Result {
        let (_authority, file, stream) = client(Completion::new(21, Err(EAGAIN))?)?;
        let owned = KBox::new(42u32, GFP_KERNEL)?;
        let (completion, returned) = stream.dequeue(move |completion| Ok((completion, owned)))?;
        assert_eq!(*returned, 42);
        assert_eq!(completion.use_id(), 21);
        assert!(matches!(completion.result(), Err(EAGAIN)));
        assert_eq!(stream.dequeue(|_| Ok(())), Err(EAGAIN));
        drop(stream);
        release(file);
        Ok(())
    }

    #[test]
    fn closing_a_stream_does_not_invoke_another_publisher() -> Result {
        let (_authority, file, mut stream) = client(Completion::new(23, Err(ECANCELED))?)?;
        stream.close()?;
        let mut called = false;
        let result = stream.dequeue(|_| {
            called = true;
            Ok(())
        });
        assert_eq!(result, Err(ENOENT));
        assert!(!called);
        drop(stream);
        release(file);
        Ok(())
    }
}
