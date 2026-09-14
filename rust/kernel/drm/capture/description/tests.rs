// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;

#[kunit_tests(rust_drm_capture_description)]
mod cases {
    use super::*;

    #[test]
    fn incomplete_metadata_does_not_form_a_description() -> Result {
        for (id, dimensions, format, modifier, requests) in [
            (0, [64, 32], fourcc::XRGB8888, 0, 8),
            (1, [0, 32], fourcc::XRGB8888, 0, 8),
            (1, [64, 0], fourcc::XRGB8888, 0, 8),
            (1, [64, 32], 0, 0, 8),
            (1, [64, 32], fourcc::XRGB8888, fourcc::FORMAT_MOD_INVALID, 8),
            (1, [64, 32], fourcc::XRGB8888, 0, 0),
        ] {
            if Description::new(id, dimensions, format, modifier, requests) != Err(EINVAL) {
                return Err(EINVAL);
            }
        }
        Ok(())
    }

    #[test]
    fn metadata_does_not_impose_a_linear_rgb_provider() -> Result {
        let description = Description::new(19, [128, 64], fourcc::NV12, 7, 8)?;
        let raw = description.raw();
        if raw.id != 19
            || raw.width != 128
            || raw.height != 64
            || raw.format != fourcc::NV12
            || raw.modifier != 7
            || raw.max_requests != 8
        {
            return Err(EINVAL);
        }
        Ok(())
    }
}
