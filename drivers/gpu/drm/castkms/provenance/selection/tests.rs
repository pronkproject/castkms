// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use kernel::prelude::*;

#[kunit_tests(rust_castkms_selection)]
mod cases {
    use super::*;

    #[test]
    fn requested_different_image_is_selected() {
        assert_eq!(
            Selection::classify(Some(2), Some(1), Some(2)),
            Selection::DifferentFramebuffer
        );
        assert_eq!(
            Selection::classify(Some(2), None, Some(2)),
            Selection::DifferentFramebuffer
        );
    }

    #[test]
    fn same_image_content_update_does_not_select() {
        assert_eq!(
            Selection::classify(Some(1), Some(1), Some(1)),
            Selection::RetainedFramebuffer
        );
    }

    #[test]
    fn derived_image_does_not_select() {
        assert_eq!(
            Selection::classify(Some(2), Some(1), Some(3)),
            Selection::RetainedFramebuffer
        );
        assert_eq!(
            Selection::classify(None, Some(1), Some(2)),
            Selection::RetainedFramebuffer
        );
    }

    #[test]
    fn blanking_does_not_select_an_image() {
        assert_eq!(
            Selection::classify(None, Some(1), None),
            Selection::RetainedFramebuffer
        );
        assert_eq!(
            Selection::classify(Some(2), Some(1), None),
            Selection::RetainedFramebuffer
        );
    }
}
