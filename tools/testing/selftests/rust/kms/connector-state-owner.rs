// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0271\]
#![no_std]

use kernel::drm::kms::connector::*;

pub fn reciprocal<C: DriverConnector, S: DriverConnectorState>() {
    fn state_of<C: DriverConnector, S: DriverConnectorState<Connector = C>>() {}
    fn connector_of<S: DriverConnectorState, C: DriverConnector<State = S>>() {}
    state_of::<C, C::State>();
    connector_of::<S, S::Connector>();
}

#[cfg(negative)]
#[derive(Clone, Default)]
pub struct Other<S>(S);

#[cfg(negative)]
impl<S: DriverConnectorState> DriverConnectorState for Other<S> {
    type Connector = S::Connector;
}
