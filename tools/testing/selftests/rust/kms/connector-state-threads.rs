// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0277\]
#![no_std]

use kernel::drm::kms::connector::*;

pub fn shared<S: DriverConnectorState>() {
    fn transferable<T: Send + Sync>() {}
    transferable::<S>();
}

#[cfg(negative)]
#[derive(Clone, Default)]
pub struct ThreadBound<C>(kernel::types::NotThreadSafe, core::marker::PhantomData<fn() -> C>);

#[cfg(negative)]
impl<C> DriverConnectorState for ThreadBound<C>
where
    C: DriverConnector<State = Self> + Clone + Default,
{
    type Connector = C;
}
