// SPDX-License-Identifier: GPL-2.0-only

//! Bounded source sampling for positioned and scaled display planes.

use kernel::prelude::*;

/// Source coordinates use unsigned 16.16 pixels; destination positions may be negative.
#[derive(Clone, Copy)]
pub(crate) struct Geometry {
    pub(crate) source: [u32; 4],
    pub(crate) position: [i32; 2],
    pub(crate) destination: [u32; 2],
    pub(crate) output: [u32; 2],
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
#[kunit_tests(rust_castkms_geometry)]
mod tests {
    use super::*;

    fn geometry() -> Geometry {
        Geometry {
            source: [0, 0, 4 << 16, 2 << 16],
            position: [0, 0],
            destination: [4, 2],
            output: [4, 2],
        }
    }

    #[test]
    fn identity_and_output_bounds() {
        let g = geometry();
        assert!(g.check(4, 2).is_ok());
        assert_eq!(g.sample(3, 1), Some((3, 1)));
        assert_eq!(g.sample(4, 1), None);
    }

    #[test]
    fn clipping_preserves_sampling_phase() {
        let mut g = geometry();
        g.position = [-1, -1];
        assert_eq!(g.sample(0, 0), Some((1, 1)));
        assert_eq!(g.sample(3, 0), None);
        g.position = [i32::MIN, i32::MAX];
        assert_eq!(g.sample(0, 0), None);
    }

    #[test]
    fn fractional_crop_and_scale_sample_centers() {
        let mut g = geometry();
        g.source = [1 << 15, 0, 3 << 16, 2 << 16];
        g.destination = [2, 2];
        assert!(g.check(4, 2).is_ok());
        assert_eq!(g.sample(0, 0), Some((1, 0)));
        assert_eq!(g.sample(1, 0), Some((2, 0)));
        g.source = [0, 0, 2 << 16, 2 << 16];
        g.destination = [4, 2];
        assert_eq!(g.sample(0, 0), Some((0, 0)));
        assert_eq!(g.sample(1, 0), Some((0, 0)));
        assert_eq!(g.sample(2, 0), Some((1, 0)));
    }

    #[test]
    fn invalid_extents_are_rejected() {
        let mut g = geometry();
        g.source[0] = u32::MAX;
        assert_eq!(g.check(4, 2), Err(EINVAL));
        g = geometry();
        g.destination[0] = 0;
        assert_eq!(g.check(4, 2), Err(EINVAL));
        g = geometry();
        g.destination[0] = 65;
        assert_eq!(g.check(4, 2), Err(EINVAL));
    }
}

impl Geometry {
    pub(crate) fn check(self, width: u32, height: u32) -> Result {
        if self.destination.contains(&0)
            || self.output.contains(&0)
            || self.output[0] > 8192
            || self.output[1] > 8192
        {
            return Err(EINVAL);
        }
        for (axis, bound) in [width, height].into_iter().enumerate() {
            let extent = u64::from(self.source[axis + 2]);
            let end = u64::from(self.source[axis]) + extent;
            if extent == 0 || end > u64::from(bound) << 16 {
                return Err(EINVAL);
            }
            // Keep both upscaling and downscaling bounded to 16:1.
            let destination = u64::from(self.destination[axis]);
            if extent < destination * (1 << 12) || extent > destination * (1 << 20) {
                return Err(EINVAL);
            }
        }
        Ok(())
    }

    /// Nearest-neighbor sampling at pixel centers, with clipping in output space.
    ///
    /// Calculations use the requested rectangles so clipping does not change the scale
    /// or sampling phase. Call `check` before using the geometry to access storage.
    pub(crate) fn sample(self, x: u32, y: u32) -> Option<(usize, usize)> {
        let mut sample = [0; 2];
        for (axis, coordinate) in [x, y].into_iter().enumerate() {
            let relative = i64::from(coordinate) - i64::from(self.position[axis]);
            let extent = u64::from(self.destination[axis]);
            if coordinate >= self.output[axis] || relative < 0 || relative as u64 >= extent {
                return None;
            }
            let source = u64::from(self.source[axis])
                + ((relative as u64 * 2 + 1) * u64::from(self.source[axis + 2])) / (extent * 2);
            sample[axis] = (source >> 16) as usize;
        }
        Some((sample[0], sample[1]))
    }

    pub(crate) fn is_identity(self, width: u32, height: u32) -> bool {
        self.position == [0, 0]
            && self.source == [0, 0, width << 16, height << 16]
            && self.destination == [width, height]
            && self.output == [width, height]
    }
}
