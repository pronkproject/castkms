// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Owned color-operation descriptions and setup for an sRGB/matrix plane pipeline.

/// An immutable operation, with no reference to mutable KMS state or property blobs.
#[derive(Clone, Copy)]
pub enum Operation {
    /// Preserve input values unchanged.
    Bypass,
    /// Decode the sRGB transfer function.
    SrgbEotf,
    /// Encode the sRGB transfer function.
    SrgbInverseEotf,
    /// Three rows of four S31.32 sign-magnitude coefficients, including offsets.
    Matrix([u64; 12]),
}
