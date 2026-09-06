// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Initial connector state created by property attachment before device-wide state creation.

use super::*;
use connector::AsRawConnector;

#[cfg(CONFIG_FAILSLAB)]
#[kunit_tests(rust_drm_connector_alloc)]
mod allocation_cases {
    use super::*;

    #[test]
    fn payload_heap_failure_allows_property_retry() -> Result {
        let ((error, absent, failures), counts) = with_fresh_connector(|connector, _, counts| {
            counts.fail_connector_heap_alloc.store(1, Ordering::Relaxed);
            let error = connector.attach_max_bpc_property(8, 12).err();
            counts.fail_connector_heap_alloc.store(0, Ordering::Relaxed);
            // SAFETY: Property attachment has returned and the device is private.
            let absent = unsafe {
                (*connector.as_raw()).state.is_null()
                    && (*connector.as_raw()).max_bpc_property.is_null()
            };
            let failures = counts.connector_heap_failures.load(Ordering::Relaxed);
            connector.attach_max_bpc_property(8, 12)?;
            Ok((error, absent, failures))
        })?;
        assert_eq!(error, Some(ENOMEM));
        assert!(absent);
        assert_eq!(failures, 1);
        assert_eq!(counts.connector_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.plane_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.crtc_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn payload_heap_failure_preserves_published_state() -> Result {
        let ((error, preserved, failures, live_states), counts) =
            with_fresh_connector(|connector, drm, counts| {
                connector.attach_max_bpc_property(8, 12)?;
                // SAFETY: Property attachment initialized matching typed state. No other
                // task accesses this unregistered device or changes its object graph.
                let initial = unsafe { (*connector.as_raw()).state };
                let connector =
                    unsafe { connector::Connector::<TestConnector>::from_raw(connector.as_raw()) };
                counts.fail_connector_heap_alloc.store(1, Ordering::Relaxed);
                let result = unsafe {
                    atomic::run_check(drm, |state| {
                        let _new = state.add_connector_state(connector)?;
                        Ok(())
                    })
                };
                counts.fail_connector_heap_alloc.store(0, Ordering::Relaxed);
                // SAFETY: Validation returned; the original state remains exclusively accessed.
                let preserved = unsafe { (*connector.as_raw()).state == initial };
                let failures = counts.connector_heap_failures.load(Ordering::Relaxed);
                let live_states = counts.connector_states.load(Ordering::Relaxed);
                // SAFETY: Same initialized, private-device lifetime as the failed check.
                unsafe {
                    atomic::run_check(drm, |state| {
                        let _new = state.add_connector_state(connector)?;
                        Ok(())
                    })
                }?;
                Ok((result.err(), preserved, failures, live_states))
            })?;
        assert_eq!(error, Some(ENOMEM));
        assert!(preserved);
        assert_eq!(failures, 1);
        assert_eq!(live_states, 2);
        assert_eq!(counts.connector_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.plane_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.crtc_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }
}

fn with_fresh_connector<R>(
    test: impl FnOnce(
        &connector::UnregisteredConnector<TestConnector>,
        &Device<TestDriver>,
        &Arc<Counts>,
    ) -> Result<R>,
) -> Result<(R, Arc<Counts>)> {
    let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
    let parent = faux::Registration::new(c"rust-kms-connector-property", None)?;
    let drm = create(parent.as_ref(), &counts, false)?;
    // SAFETY: Mode configuration is initialized, with no registration or concurrent users.
    // Add one connector before any transactions; do not reinitialize the mode configuration.
    let setup = unsafe { UnregisteredKmsDevice::new(&drm) };
    let connector = connector::UnregisteredConnector::<TestConnector>::new(
        &setup,
        connector::Type::Virtual,
        (),
    )?;
    let result = test(connector, &drm, &counts);
    drop(drm);
    drop(parent);
    Ok((result?, counts))
}

#[kunit_tests(rust_drm_connector_properties)]
mod cases {
    use super::*;

    #[test]
    fn invalid_max_bpc_range_leaves_state_uninitialized() -> Result {
        let ((errors, state_absent, property_absent, live_states), counts) =
            with_fresh_connector(|connector, _, counts| {
                let errors = [(0, 8), (12, 8), (8, u32::MAX)]
                    .map(|(min, max)| connector.attach_max_bpc_property(min, max).err());
                // SAFETY: The initialized connector is owned by the exclusively accessed device.
                let (state_absent, property_absent) = unsafe {
                    (
                        (*connector.as_raw()).state.is_null(),
                        (*connector.as_raw()).max_bpc_property.is_null(),
                    )
                };
                Ok((
                    errors,
                    state_absent,
                    property_absent,
                    counts.connector_states.load(Ordering::Relaxed),
                ))
            })?;
        assert_eq!(errors, [Some(EINVAL); 3]);
        assert!(state_absent);
        assert!(property_absent);
        // The original connector has state; the added connector must not allocate any.
        assert_eq!(live_states, 1);
        assert_eq!(counts.connector_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.plane_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.crtc_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn max_bpc_state_failure_allows_same_connector_retry() -> Result {
        let ((error, state_absent, property_absent, failed_states, requested, maximum), counts) =
            with_fresh_connector(|connector, _, counts| {
                counts
                    .fail_connector_state_alloc
                    .store(1, Ordering::Relaxed);
                let error = connector.attach_max_bpc_property(8, 12).err();
                // SAFETY: The callback has returned; no other task accesses the connector.
                let (state_absent, property_absent) = unsafe {
                    (
                        (*connector.as_raw()).state.is_null(),
                        (*connector.as_raw()).max_bpc_property.is_null(),
                    )
                };
                let failed_states = counts.connector_states.load(Ordering::Relaxed);
                counts
                    .fail_connector_state_alloc
                    .store(0, Ordering::Relaxed);
                connector.attach_max_bpc_property(8, 12)?;
                // SAFETY: Successful attachment initializes the connector state. The device
                // stays unregistered and exclusively accessed throughout this callback.
                let state = unsafe { &*(*connector.as_raw()).state };
                Ok((
                    error,
                    state_absent,
                    property_absent,
                    failed_states,
                    state.max_requested_bpc,
                    state.max_bpc,
                ))
            })?;
        assert_eq!(error, Some(ENOMEM));
        assert!(state_absent);
        assert!(property_absent);
        assert_eq!(failed_states, 1);
        assert_eq!((requested, maximum), (12, 12));
        assert_eq!(counts.connector_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.plane_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.crtc_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn max_bpc_state_survives_device_state_creation() -> Result {
        let ((preserved, requested, maximum, live_states), counts) =
            with_fresh_connector(|connector, drm, counts| {
                connector.attach_max_bpc_property(8, 12)?;
                // SAFETY: Property attachment created state for the new connector. Existing
                // objects remain private, with no transactions, registration or other users.
                let before = unsafe { (*connector.as_raw()).state };
                crate::error::to_result(unsafe {
                    bindings::drm_mode_config_create_initial_state(drm.as_raw())
                })?;
                // SAFETY: State creation succeeded and no other task can replace the state.
                let after = unsafe { (*connector.as_raw()).state };
                let state = unsafe { &*after };
                Ok((
                    before == after,
                    state.max_requested_bpc,
                    state.max_bpc,
                    counts.connector_states.load(Ordering::Relaxed),
                ))
            })?;
        assert!(preserved);
        assert_eq!((requested, maximum), (12, 12));
        assert_eq!(live_states, 2);
        assert_eq!(counts.connector_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.plane_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.crtc_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }
}
