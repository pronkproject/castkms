// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Tiled connector metadata and native tile-group ownership.

use super::*;
use crate::drm::kms::{connector::AsRawConnector, testing::TestDevice};

#[kunit_tests(rust_drm_kms_tiles)]
mod cases {
    use super::*;

    #[test]
    fn tile_geometry_rejects_empty_and_out_of_grid_values() {
        assert_eq!(
            connector::Tile::new(0, 1, 0, 0, 640, 480, true),
            Err(EINVAL)
        );
        assert_eq!(
            connector::Tile::new(2, 1, 2, 0, 640, 480, true),
            Err(EINVAL)
        );
        assert_eq!(
            connector::Tile::new(2, 1, 0, 1, 640, 480, true),
            Err(EINVAL)
        );
        assert_eq!(connector::Tile::new(2, 1, 0, 0, 0, 480, true), Err(EINVAL));
        assert!(connector::Tile::new(2, 1, 1, 0, 640, 480, true).is_ok());
    }

    #[test]
    fn connectors_share_one_group_and_publish_standard_metadata() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        counts.output_count.store(2, Ordering::Relaxed);
        counts.tile_topology.store(1, Ordering::Relaxed);
        let parent = faux::Registration::new(c"rust-kms-tiles", None)?;
        let dev = TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let left = dev.connector_at(0)?.as_raw();
        let right = dev.connector_at(1)?.as_raw();

        // SAFETY: The fixture retains both initialized connectors and excludes topology changes.
        let metadata = unsafe {
            (
                (*left).has_tile,
                (*right).has_tile,
                (*left).tile_group,
                (*right).tile_group,
                (*left).num_h_tile,
                (*right).num_h_tile,
                (*left).num_v_tile,
                (*right).num_v_tile,
                (*left).tile_h_loc,
                (*right).tile_h_loc,
                (*left).tile_h_size,
                (*right).tile_v_size,
                (*left).tile_blob_ptr,
                (*right).tile_blob_ptr,
            )
        };
        assert!(metadata.0);
        assert!(metadata.1);
        assert_eq!(metadata.2, metadata.3);
        assert!(!metadata.2.is_null());
        // SAFETY: The preceding assertion proved that this retained connector group is non-null.
        assert!(unsafe { (*metadata.2).id } > 0);
        assert_eq!(metadata.4, 2);
        assert_eq!(metadata.5, 2);
        assert_eq!(metadata.6, 1);
        assert_eq!(metadata.7, 1);
        assert_eq!(metadata.8, 0);
        assert_eq!(metadata.9, 1);
        assert_eq!(metadata.10, 640);
        assert_eq!(metadata.11, 480);
        assert!(!metadata.12.is_null());
        assert!(!metadata.13.is_null());

        drop(dev);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }
}
