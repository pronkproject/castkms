// SPDX-License-Identifier: GPL-2.0-only

//! Completion of private CPU capture jobs from independently retained host images.

use crate::host_compositor::compose::Completed;
use kernel::drm::capture::Job;

/// Finish a CPU job with packed XRGB pixels and defined unused bytes.
///
/// The caller must authorize the image and recipient before calling. No scanout source is
/// claimed here. Job storage stays private until successful completion, including during the
/// copy and the pass that sets each unused X byte to 0xff. Page padding is not copied.
///
/// This is not a copy into an exported DMA-BUF: its owner could observe bytes before completion.
/// Cancellation or revocation may still suppress delivery when the job is completed.
pub(crate) fn complete(image: &Completed, mut job: Job) {
    let pixels = job.data_mut();
    let status = image.copy_pixels(pixels);
    if status.is_ok() {
        // A successful exact-size copy has four bytes per packed XRGB pixel.
        for pixel in pixels.chunks_exact_mut(4) {
            pixel[3] = 0xff;
        }
    }
    job.complete(status);
}
