// SPDX-License-Identifier: GPL-2.0-only

//! Connector CEC adapter state and the userspace-backed transport queue.

use crate::display;
use kernel::{
    bindings,
    drm::kms::connector::{
        cec::{Message, TransmitResult},
        Connector,
    },
    prelude::*,
    sync::{aref::ARef, poll::PollCondVar, Arc, Mutex},
    workqueue::{self, impl_has_delayed_work, new_delayed_work, DelayedWork, WorkItem},
};

const TRANSMIT_TIMEOUT_MS: u32 = 2_000;

#[derive(Clone, Copy)]
pub(crate) struct Transaction {
    pub(crate) cookie: u64,
    pub(crate) state_generation: u64,
    pub(crate) signal_free_time: u32,
    pub(crate) attempts: u8,
    pub(crate) message: Message,
}

struct Pending {
    transaction: Transaction,
    acquired: bool,
    timeout: Arc<Timeout>,
}

#[derive(Clone, Copy)]
pub(crate) struct Snapshot {
    pub(crate) generation: u64,
    pub(crate) flags: u32,
    pub(crate) physical_address: u16,
    pub(crate) logical_address_mask: u16,
    pub(crate) pending_cookie: u64,
    pub(crate) submitted: u64,
    pub(crate) completed: u64,
    pub(crate) nack: u64,
    pub(crate) error: u64,
    pub(crate) timeout: u64,
    pub(crate) received: u64,
    pub(crate) invalid: u64,
}

struct State {
    connector: Option<ARef<Connector<display::Connector>>>,
    closed: bool,
    online: bool,
    attached: bool,
    enabled: bool,
    generation: u64,
    next_cookie: u64,
    logical_address_mask: u16,
    pending: Option<Pending>,
    submitted: u64,
    completed: u64,
    nack: u64,
    error: u64,
    timeout: u64,
    received: u64,
    invalid: u64,
}

#[pin_data]
pub(crate) struct Cec {
    #[pin]
    state: Mutex<State>,
    #[pin]
    address_update: Mutex<()>,
    #[pin]
    changed: PollCondVar,
}

impl Cec {
    pub(crate) fn new() -> Result<Arc<Self>> {
        Arc::pin_init(
            pin_init!(Self {
                state <- kernel::new_mutex!(State {
                    connector: None,
                    closed: false,
                    online: false,
                    attached: false,
                    enabled: false,
                    generation: 1,
                    next_cookie: 1,
                    logical_address_mask: 0,
                    pending: None,
                    submitted: 0,
                    completed: 0,
                    nack: 0,
                    error: 0,
                    timeout: 0,
                    received: 0,
                    invalid: 0,
                }),
                address_update <- kernel::new_mutex!(()),
                changed <- kernel::new_poll_condvar!(),
            }),
            GFP_KERNEL,
        )
    }

    pub(crate) fn install(&self, connector: ARef<Connector<display::Connector>>) -> Result {
        let mut state = self.state.lock();
        if state.closed {
            return Err(ENODEV);
        }
        if state.connector.is_some() {
            return Err(EEXIST);
        }
        state.connector = Some(connector);
        Ok(())
    }

    pub(crate) fn changed(&self) -> &PollCondVar {
        &self.changed
    }

    pub(crate) fn readable(&self) -> Result<bool> {
        let state = self.state.lock();
        if state.closed {
            Err(ECANCELED)
        } else {
            Ok(state
                .pending
                .as_ref()
                .is_some_and(|pending| !pending.acquired))
        }
    }

    fn bump(state: &mut State) {
        state.generation = state.generation.saturating_add(1);
    }

    fn abort_locked(
        state: &mut State,
        timeout: bool,
    ) -> Option<(ARef<Connector<display::Connector>>, Arc<Timeout>)> {
        let pending = state.pending.take()?;
        state.error = state.error.saturating_add(1);
        if timeout {
            state.timeout = state.timeout.saturating_add(1);
        }
        Self::bump(state);
        Some((state.connector.clone()?, pending.timeout))
    }

    fn report_abort(
        aborted: Option<(ARef<Connector<display::Connector>>, Arc<Timeout>)>,
        flush_timeout: bool,
    ) {
        if let Some((connector, timeout)) = aborted {
            connector.cec_transmit_done(TransmitResult {
                status: (bindings::CEC_TX_STATUS_ERROR | bindings::CEC_TX_STATUS_MAX_RETRIES) as u8,
                error: 1,
                ..Default::default()
            });
            if flush_timeout {
                timeout.work.flush();
            }
        }
    }

    pub(crate) fn refresh_physical_address(&self) {
        // Serialize the native adapter update with other refreshers and close.
        // Do not hold `state` across the native call: CEC callbacks may acquire it
        // while the adapter serializes its own address change.
        let _update = self.address_update.lock();
        let (connector, valid) = {
            let state = self.state.lock();
            let Some(connector) = state.connector.clone() else {
                return;
            };
            let valid = !state.closed
                && state.online
                && state.attached
                && connector.cec_physical_address() != bindings::CEC_PHYS_ADDR_INVALID as u16;
            (connector, valid)
        };
        connector.cec_set_physical_address_valid(valid);
    }

    pub(crate) fn set_online(&self, online: bool) -> Result {
        let aborted = {
            let mut state = self.state.lock();
            if state.closed {
                return Err(ECANCELED);
            }
            if state.online == online {
                return Ok(());
            }
            state.online = online;
            Self::bump(&mut state);
            if online {
                None
            } else {
                Self::abort_locked(&mut state, false)
            }
        };
        Self::report_abort(aborted, true);
        self.refresh_physical_address();
        self.changed.notify_all();
        Ok(())
    }

    pub(crate) fn set_attached(&self, attached: bool) {
        let aborted = {
            let mut state = self.state.lock();
            if state.closed || (!attached && !state.attached) {
                return;
            }
            state.attached = attached;
            Self::bump(&mut state);
            // A replacement is a different physical CEC sink even though the
            // connector remains attached. Never carry its transaction across.
            Self::abort_locked(&mut state, false)
        };
        Self::report_abort(aborted, true);
        self.refresh_physical_address();
        self.changed.notify_all();
    }

    pub(crate) fn reset(&self) {
        let aborted = {
            let mut state = self.state.lock();
            state.online = false;
            state.attached = false;
            Self::bump(&mut state);
            Self::abort_locked(&mut state, false)
        };
        Self::report_abort(aborted, true);
        self.refresh_physical_address();
        self.changed.notify_all();
    }

    pub(crate) fn close(&self) {
        let (aborted, connector) = {
            let mut state = self.state.lock();
            if state.closed {
                return;
            }
            state.closed = true;
            state.online = false;
            state.attached = false;
            let aborted = Self::abort_locked(&mut state, false);
            let connector = state.connector.clone();
            state.connector = None;
            (aborted, connector)
        };
        Self::report_abort(aborted, true);
        let _update = self.address_update.lock();
        if let Some(connector) = connector {
            connector.cec_set_physical_address_valid(false);
        }
        self.changed.notify_all();
    }

    pub(crate) fn enable(&self, enabled: bool) -> Result {
        let aborted = {
            let mut state = self.state.lock();
            if state.closed {
                return Err(ENODEV);
            }
            if state.enabled == enabled {
                return Ok(());
            }
            state.enabled = enabled;
            Self::bump(&mut state);
            if enabled {
                None
            } else {
                Self::abort_locked(&mut state, false)
            }
        };
        // The CEC core invokes enable while holding its adapter mutex and
        // cancels its own transmission after the callback returns. Reporting
        // completion or synchronously flushing a timeout which is waiting for
        // that mutex would deadlock.
        drop(aborted);
        self.changed.notify_all();
        Ok(())
    }

    pub(crate) fn logical_address(&self, address: u8) -> Result {
        let mut state = self.state.lock();
        if state.closed {
            return Err(ENODEV);
        }
        if address == bindings::CEC_LOG_ADDR_INVALID as u8 {
            state.logical_address_mask = 0;
        } else if address < 16 {
            state.logical_address_mask |= 1 << address;
        } else {
            return Err(EINVAL);
        }
        Self::bump(&mut state);
        drop(state);
        self.changed.notify_all();
        Ok(())
    }

    pub(crate) fn transmit(
        self: &Arc<Self>,
        attempts: u8,
        signal_free_time: u32,
        message: Message,
    ) -> Result {
        {
            let mut state = self.state.lock();
            if state.closed {
                return Err(ENODEV);
            }
            if !state.online || !state.attached || !state.enabled {
                return Err(ENONET);
            }
            if state.pending.is_some() {
                return Err(EBUSY);
            }
            // The CEC core does not call adap_log_addr for the unregistered
            // address. An outbound frame nevertheless proves that its
            // initiator is local and must participate in reflection defense.
            let initiator = message.as_bytes()[0] >> 4;
            let initiator_bit = 1 << initiator;
            if state.logical_address_mask & initiator_bit == 0 {
                state.logical_address_mask |= initiator_bit;
                Self::bump(&mut state);
            }
            let cookie = state.next_cookie;
            let next_cookie = cookie.checked_add(1).ok_or(EOVERFLOW)?;
            let timeout = Timeout::new(self.clone(), cookie)?;
            workqueue::system_dfl()
                .enqueue_delayed(
                    timeout.clone(),
                    kernel::time::msecs_to_jiffies(TRANSMIT_TIMEOUT_MS),
                )
                .map_err(|_| EBUSY)?;
            state.next_cookie = next_cookie;
            let transaction = Transaction {
                cookie,
                state_generation: state.generation,
                signal_free_time,
                attempts,
                message,
            };
            state.pending = Some(Pending {
                transaction,
                acquired: false,
                timeout,
            });
            state.submitted = state.submitted.saturating_add(1);
        }
        self.changed.notify_all();
        Ok(())
    }

    pub(crate) fn peek(&self) -> Result<Transaction> {
        let state = self.state.lock();
        if state.closed {
            return Err(ECANCELED);
        }
        let pending = state.pending.as_ref().ok_or(EAGAIN)?;
        if pending.acquired {
            return Err(EAGAIN);
        }
        Ok(pending.transaction)
    }

    pub(crate) fn acquire(&self, cookie: u64) -> Result {
        let mut state = self.state.lock();
        if state.closed {
            return Err(ECANCELED);
        }
        let pending = state.pending.as_mut().ok_or(ESTALE)?;
        if pending.transaction.cookie != cookie {
            return Err(ESTALE);
        }
        if pending.acquired {
            return Err(EAGAIN);
        }
        pending.acquired = true;
        Ok(())
    }

    pub(crate) fn complete(&self, cookie: u64, result: TransmitResult) -> Result {
        const VALID_STATUS: u8 = (bindings::CEC_TX_STATUS_OK
            | bindings::CEC_TX_STATUS_ARB_LOST
            | bindings::CEC_TX_STATUS_NACK
            | bindings::CEC_TX_STATUS_LOW_DRIVE
            | bindings::CEC_TX_STATUS_ERROR
            | bindings::CEC_TX_STATUS_MAX_RETRIES
            | bindings::CEC_TX_STATUS_ABORTED
            | bindings::CEC_TX_STATUS_TIMEOUT) as u8;
        if result.status == 0 || result.status & !VALID_STATUS != 0 {
            return Err(EINVAL);
        }
        let (connector, timeout) = {
            let mut state = self.state.lock();
            if state.closed {
                return Err(ECANCELED);
            }
            let connector = state.connector.clone().ok_or(ENODEV)?;
            let pending = state.pending.as_ref().ok_or(ENOENT)?;
            if pending.transaction.cookie != cookie {
                state.invalid = state.invalid.saturating_add(1);
                return Err(ESTALE);
            }
            if !pending.acquired {
                return Err(EAGAIN);
            }
            let pending = state.pending.take().ok_or(ENOENT)?;
            if result.status & bindings::CEC_TX_STATUS_OK as u8 != 0 {
                state.completed = state.completed.saturating_add(1);
            } else if result.status & bindings::CEC_TX_STATUS_NACK as u8 != 0 {
                state.nack = state.nack.saturating_add(1);
            } else {
                state.error = state.error.saturating_add(1);
            }
            Self::bump(&mut state);
            (connector, pending.timeout)
        };
        connector.cec_transmit_done(result);
        timeout.work.flush();
        self.changed.notify_all();
        Ok(())
    }

    pub(crate) fn receive(&self, message: Message) -> Result {
        let connector = {
            let mut state = self.state.lock();
            if state.closed {
                return Err(ECANCELED);
            }
            if !state.online {
                state.invalid = state.invalid.saturating_add(1);
                return Err(ENONET);
            }
            if !state.attached {
                state.invalid = state.invalid.saturating_add(1);
                return Err(ENOTCONN);
            }
            let connector = state.connector.clone().ok_or(ENODEV)?;
            let initiator = message.as_bytes()[0] >> 4;
            if state.logical_address_mask & (1 << initiator) != 0 {
                state.invalid = state.invalid.saturating_add(1);
                return Err(EINVAL);
            }
            state.received = state.received.saturating_add(1);
            connector
        };
        connector.cec_received(message);
        Ok(())
    }

    pub(crate) fn snapshot(&self) -> Result<Snapshot> {
        let state = self.state.lock();
        if state.closed {
            return Err(ECANCELED);
        }
        let connector = state.connector.as_ref().ok_or(ENODEV)?;
        let mut flags = 0;
        if state.online {
            flags |= kernel::uapi::DRM_CASTKMS_CEC_STATE_ONLINE;
        }
        if state.attached {
            flags |= kernel::uapi::DRM_CASTKMS_CEC_STATE_MONITOR_ATTACHED;
        }
        if state.enabled {
            flags |= kernel::uapi::DRM_CASTKMS_CEC_STATE_ADAPTER_ENABLED;
        }
        Ok(Snapshot {
            generation: state.generation,
            flags,
            physical_address: connector.cec_physical_address(),
            logical_address_mask: state.logical_address_mask,
            pending_cookie: state
                .pending
                .as_ref()
                .map_or(0, |pending| pending.transaction.cookie),
            submitted: state.submitted,
            completed: state.completed,
            nack: state.nack,
            error: state.error,
            timeout: state.timeout,
            received: state.received,
            invalid: state.invalid,
        })
    }

    fn timeout(&self, cookie: u64) {
        let connector = {
            let mut state = self.state.lock();
            if !state
                .pending
                .as_ref()
                .is_some_and(|pending| pending.transaction.cookie == cookie)
            {
                return;
            }
            Self::abort_locked(&mut state, true)
        };
        Self::report_abort(connector, false);
        self.changed.notify_all();
    }
}

#[pin_data]
struct Timeout {
    cec: Arc<Cec>,
    cookie: u64,
    #[pin]
    work: DelayedWork<Self>,
}

impl_has_delayed_work! {
    impl HasDelayedWork<Self> for Timeout { self.work }
}

impl Timeout {
    fn new(cec: Arc<Cec>, cookie: u64) -> Result<Arc<Self>> {
        Arc::pin_init(
            pin_init!(Self {
                cec,
                cookie,
                work <- new_delayed_work!("castkms-cec-timeout"),
            }),
            GFP_KERNEL,
        )
    }
}

impl WorkItem for Timeout {
    type Pointer = Arc<Self>;

    fn run(timeout: Arc<Self>) {
        timeout.cec.timeout(timeout.cookie);
    }
}
