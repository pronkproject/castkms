// SPDX-License-Identifier: GPL-2.0-only

//! Bounded, frame-aligned private PCM storage with irreversible termination.

use super::{
    BUFFER_FRAMES,
    FRAME_BYTES, //
};
use kernel::prelude::*;

pub(super) struct Buffer {
    bytes: KVVec<u8>,
    head: usize,
    len: usize,
    dropped: u64,
    terminal: Option<Error>,
}

impl Buffer {
    pub(super) fn new() -> Result<Self> {
        let mut bytes = KVVec::with_capacity(BUFFER_FRAMES * FRAME_BYTES, GFP_KERNEL)?;
        bytes.resize(BUFFER_FRAMES * FRAME_BYTES, 0, GFP_KERNEL)?;
        Ok(Self {
            bytes,
            head: 0,
            len: 0,
            dropped: 0,
            terminal: None,
        })
    }

    pub(super) fn push(&mut self, input: &[u8]) -> Result {
        if let Some(error) = self.terminal {
            return Err(error);
        }
        if input.len() % FRAME_BYTES != 0 || input.len() > self.bytes.len() {
            return Err(EINVAL);
        }
        if input.len() > self.bytes.len() - self.len {
            self.account_dropped((self.len / FRAME_BYTES) as u64);
            self.head = 0;
            self.len = 0;
        }
        let tail = (self.head + self.len) % self.bytes.len();
        let first = input.len().min(self.bytes.len() - tail);
        self.bytes[tail..tail + first].copy_from_slice(&input[..first]);
        self.bytes[..input.len() - first].copy_from_slice(&input[first..]);
        self.len += input.len();
        Ok(())
    }

    /// Copy whole frames into kernel-owned storage; the file adapter handles user access.
    pub(super) fn pop(&mut self, output: &mut [u8]) -> Result<usize> {
        if let Some(error) = self.terminal {
            return Err(error);
        }
        if output.len() < FRAME_BYTES {
            return Err(EINVAL);
        }
        if self.len == 0 {
            return Err(EAGAIN);
        }
        let count = self.len.min(output.len() / FRAME_BYTES * FRAME_BYTES);
        let first = count.min(self.bytes.len() - self.head);
        output[..first].copy_from_slice(&self.bytes[self.head..self.head + first]);
        output[first..count].copy_from_slice(&self.bytes[..count - first]);
        self.head = (self.head + count) % self.bytes.len();
        self.len -= count;
        Ok(count)
    }

    pub(super) fn account_dropped(&mut self, frames: u64) {
        self.dropped = self.dropped.saturating_add(frames);
    }

    pub(super) fn discard(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    pub(super) fn dropped(&self) -> u64 {
        self.dropped
    }

    pub(super) fn readable(&self) -> bool {
        self.terminal.is_none() && self.len != 0
    }

    pub(super) fn terminal(&self) -> Option<Error> {
        self.terminal
    }

    pub(super) fn terminate(&mut self, error: Error) {
        self.terminal.get_or_insert(error);
        self.head = 0;
        self.len = 0;
    }
}
