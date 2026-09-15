// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use kernel::prelude::*;

fn check(value: bool) -> Result {
    if value {
        Ok(())
    } else {
        Err(EINVAL)
    }
}

#[kunit_tests(rust_castkms_audio)]
mod cases {
    use super::*;

    #[test]
    fn ring_reads_whole_frames_and_preserves_order() -> Result {
        let mut ring = buffer::Buffer::new()?;
        let mut output = [0; 9];
        check(matches!(ring.pop(&mut output), Err(EAGAIN)))?;
        ring.push(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12])?;
        check(ring.pop(&mut output)? == 8)?;
        check(output == [1, 2, 3, 4, 5, 6, 7, 8, 0])?;
        check(ring.pop(&mut output)? == 4)?;
        check(output[..4] == [9, 10, 11, 12])?;
        check(!ring.readable())
    }

    #[test]
    fn discard_flushes_samples_without_terminating_capture() -> Result {
        let mut ring = buffer::Buffer::new()?;
        let mut output = [0; 4];
        ring.push(&[1; 4])?;
        ring.discard();
        check(!ring.readable() && ring.dropped() == 0)?;
        check(matches!(ring.pop(&mut output), Err(EAGAIN)))?;
        ring.push(&[2; 4])?;
        check(ring.pop(&mut output)? == 4 && output == [2; 4])
    }

    #[test]
    fn ring_wraps_without_losing_or_reordering_samples() -> Result {
        let mut ring = buffer::Buffer::new()?;
        let mut output = [0; 4];
        for _ in 0..BUFFER_FRAMES - 1 {
            ring.push(&[1; 4])?;
            ring.pop(&mut output)?;
        }
        ring.push(&[2, 3, 4, 5, 6, 7, 8, 9])?;
        ring.pop(&mut output)?;
        check(output == [2, 3, 4, 5])?;
        ring.pop(&mut output)?;
        check(output == [6, 7, 8, 9] && ring.dropped() == 0)
    }

    #[test]
    fn overflow_discards_old_audio_instead_of_increasing_latency() -> Result {
        let mut ring = buffer::Buffer::new()?;
        for _ in 0..BUFFER_FRAMES {
            ring.push(&[1; 4])?;
        }
        ring.push(&[2; 4])?;
        check(ring.dropped() == BUFFER_FRAMES as u64)?;
        let mut output = [0; 8];
        check(ring.pop(&mut output)? == 4)?;
        check(output[..4] == [2; 4] && !ring.readable())
    }

    #[test]
    fn terminal_state_discards_queued_data_and_cannot_be_overridden() -> Result {
        let mut ring = buffer::Buffer::new()?;
        ring.push(&[1; 4])?;
        ring.terminate(ESTALE);
        ring.terminate(ENODEV);
        check(!ring.readable() && ring.terminal() == Some(ESTALE))?;
        check(matches!(ring.pop(&mut [0; 4]), Err(ESTALE)))?;
        check(matches!(ring.push(&[2; 4]), Err(ESTALE)))
    }

    #[test]
    fn invalid_frame_sizes_do_not_consume_or_publish_data() -> Result {
        let mut ring = buffer::Buffer::new()?;
        check(matches!(ring.push(&[1; 3]), Err(EINVAL)))?;
        check(!ring.readable())?;
        ring.push(&[1; 4])?;
        check(matches!(ring.pop(&mut [0; 3]), Err(EINVAL)))?;
        check(ring.readable())
    }
}
