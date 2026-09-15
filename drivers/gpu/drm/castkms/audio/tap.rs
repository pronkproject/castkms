// SPDX-License-Identifier: GPL-2.0-only

//! One private, continuously clocked PCM stream.

use super::{
    buffer::Buffer,
    clock::Capture,
    playback::{Cursor, Playback},
    FRAME_BYTES,
    PERIOD_FRAMES, //
};
use kernel::{
    prelude::*,
    sync::{
        poll::PollCondVar,
        Arc,
        ArcBorrow,
        SpinLockIrq, //
    },
    time::{
        hrtimer::{
            ArcHrTimerHandle,
            HrTimer,
            HrTimerCallback,
            HrTimerCallbackContext,
            HrTimerPointer,
            HrTimerRestart,
            RelativeMode, //
        },
        Delta,
        Instant,
        Monotonic, //
    }, //
};

struct State {
    buffer: Buffer,
    clock: Capture,
    cursor: Cursor,
    scratch: KVec<u8>,
    link_generation: u64,
}

#[pin_data]
pub(super) struct Tap {
    playback: Arc<Playback>,
    born: Instant<Monotonic>,
    #[pin]
    state: SpinLockIrq<State>,
    #[pin]
    timer: HrTimer<Self>,
    #[pin]
    pub(super) changed: PollCondVar,
}

kernel::impl_has_hr_timer! {
    impl HasHrTimer<Self> for Tap {
        mode: RelativeMode<Monotonic>,
        field: self.timer,
    }
}

impl Tap {
    pub(super) fn new(playback: Arc<Playback>) -> Result<Arc<Self>> {
        let buffer = Buffer::new()?;
        let mut scratch = KVec::with_capacity(4 * PERIOD_FRAMES * FRAME_BYTES, GFP_KERNEL)?;
        scratch.resize(4 * PERIOD_FRAMES * FRAME_BYTES, 0, GFP_KERNEL)?;
        Arc::pin_init(
            pin_init!(Self {
                playback,
                born: Instant::now(),
                state <- kernel::new_spinlock_irq!(State {
                    buffer, clock: Capture::new(0), cursor: Cursor::default(),
                    scratch,
                    link_generation: 0,
                }),
                timer <- HrTimer::new(),
                changed <- kernel::new_poll_condvar!(),
            }),
            GFP_KERNEL,
        )
    }

    pub(super) fn start(self: &Arc<Self>) -> ArcHrTimerHandle<Self> {
        self.clone().start(Delta::from_millis(10))
    }

    pub(super) fn terminate(&self, error: Error) {
        self.state.lock().buffer.terminate(error);
        self.changed.notify_all();
    }

    pub(super) fn terminal(&self) -> Option<Error> {
        self.state.lock().buffer.terminal()
    }

    pub(super) fn readable(&self) -> bool {
        let mut state = self.state.lock();
        self.refresh_link(&mut state) && state.buffer.readable()
    }

    pub(super) fn dropped(&self) -> u64 {
        self.state.lock().buffer.dropped()
    }

    pub(super) fn read(&self, output: &mut [u8]) -> Result<usize> {
        let mut state = self.state.lock();
        if let Some(error) = state.buffer.terminal() {
            return Err(error);
        }
        if !self.refresh_link(&mut state) {
            return Err(EAGAIN);
        }
        state.buffer.pop(output)
    }

    fn refresh_link(&self, state: &mut State) -> bool {
        let (enabled, generation) = self.playback.availability();
        if state.link_generation != generation {
            state.buffer.discard();
            state.link_generation = generation;
        }
        enabled
    }

    /// Wait without holding display or authority locks; callers may serialize their readers.
    pub(super) fn wait(&self) -> Result {
        let mut state = self.state.lock();
        while !state.buffer.readable() && state.buffer.terminal().is_none() {
            if self.changed.wait_interruptible(&mut state) {
                return Err(ERESTARTSYS);
            }
        }
        Ok(())
    }
}

impl HrTimerCallback for Tap {
    type Pointer<'a> = Arc<Self>;

    fn run(
        this: ArcBorrow<'_, Self>,
        mut context: HrTimerCallbackContext<'_, Self>,
    ) -> HrTimerRestart {
        {
            let mut state = this.state.lock();
            let state = &mut *state;
            if state.buffer.terminal().is_some() {
                return HrTimerRestart::NoRestart;
            }
            let due = state
                .clock
                .advance(this.born.elapsed().as_nanos().max(0) as u64);
            let enabled = this.refresh_link(state);
            if enabled {
                state.buffer.account_dropped(due.dropped);
            }
            if enabled && due.frames != 0 {
                let scratch = &mut state.scratch[..due.frames * FRAME_BYTES];
                match this.playback.read(&mut state.cursor, scratch) {
                    Ok(()) => {
                        let _ = state.buffer.push(scratch);
                    }
                    Err(error) => state.buffer.terminate(error),
                }
            }
        }
        this.changed.notify_all();
        context.forward_now(Delta::from_millis(10));
        HrTimerRestart::Restart
    }
}
