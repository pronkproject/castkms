// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use kernel::prelude::*;

#[kunit_tests(rust_castkms_provenance)]
mod tests {
    use super::*;

    #[test]
    fn same_image_preserves_adopted_owner_after_handoff() {
        let provenance = Provenance {
            origin: Origin::Creator(1),
        };
        assert_eq!(
            Provenance::for_update(
                Some(&provenance),
                PreviousOwner::SameFramebuffer(Some(&2)),
                Some(&3),
                Selection::RetainedFramebuffer
            ),
            Some(&2)
        );
        assert_eq!(
            Provenance::for_update(
                Some(&provenance),
                PreviousOwner::SameFramebuffer(Some(&2)),
                None,
                Selection::RetainedFramebuffer
            ),
            Some(&2)
        );
    }

    #[test]
    fn same_image_does_not_gain_an_owner_when_master_appears() {
        let provenance = Provenance {
            origin: Origin::Associated(1),
        };
        assert_eq!(
            Provenance::for_update(
                Some(&provenance),
                PreviousOwner::SameFramebuffer(None),
                Some(&1),
                Selection::RetainedFramebuffer
            ),
            None
        );
    }

    #[test]
    fn replacement_resolves_selection_independently_of_creator() {
        let provenance = Provenance {
            origin: Origin::Creator(1),
        };
        assert_eq!(
            Provenance::for_update(
                Some(&provenance),
                PreviousOwner::DifferentFramebuffer,
                Some(&2),
                Selection::DifferentFramebuffer
            ),
            Some(&2)
        );
        assert_eq!(
            Provenance::for_update(
                Some(&provenance),
                PreviousOwner::DifferentFramebuffer,
                Some(&2),
                Selection::RetainedFramebuffer
            ),
            Some(&1)
        );
        assert_eq!(provenance.owner(None), Some(&1));
    }

    #[test]
    fn absent_metadata_requires_explicit_selection_for_adoption() {
        assert_eq!(
            Provenance::for_update(
                None,
                PreviousOwner::DifferentFramebuffer,
                Some(&2),
                Selection::DifferentFramebuffer
            ),
            Some(&2)
        );
        assert_eq!(
            Provenance::for_update(
                None,
                PreviousOwner::DifferentFramebuffer,
                Some(&2),
                Selection::RetainedFramebuffer
            ),
            None
        );
    }

    #[test]
    fn creator_survives_handoff_without_adoption() {
        let provenance = Provenance {
            origin: Origin::Creator(1),
        };
        assert_eq!(provenance.owner(Some(&2)), Some(&1));
        assert_eq!(
            provenance.committed_owner(Some(&2), Selection::RetainedFramebuffer),
            Some(&1)
        );
        assert_eq!(provenance.owner(None), Some(&1));
    }

    #[test]
    fn association_requires_matching_current_identity() {
        let provenance = Provenance {
            origin: Origin::Associated(1),
        };
        assert_eq!(provenance.owner(Some(&1)), Some(&1));
        assert_eq!(provenance.owner(Some(&2)), None);
        assert_eq!(provenance.owner(None), None);
    }

    #[test]
    fn different_selection_adopts_without_mutating_creation_evidence() {
        let provenance = Provenance {
            origin: Origin::Creator(1),
        };
        assert_eq!(
            provenance.committed_owner(Some(&2), Selection::DifferentFramebuffer),
            Some(&2)
        );
        assert_eq!(provenance.owner(Some(&2)), Some(&1));
        assert_eq!(
            provenance.committed_owner(None, Selection::DifferentFramebuffer),
            Some(&1)
        );
    }

    #[test]
    fn unknown_creation_is_not_implicitly_owned() {
        let provenance = Provenance {
            origin: Origin::<u32>::Unknown,
        };
        assert_eq!(
            provenance.committed_owner(Some(&2), Selection::RetainedFramebuffer),
            None
        );
        assert_eq!(
            provenance.committed_owner(Some(&2), Selection::DifferentFramebuffer),
            Some(&2)
        );
        assert_eq!(
            provenance.committed_owner(None, Selection::DifferentFramebuffer),
            None
        );
    }
}
