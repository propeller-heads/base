//! Optional build-event stream emitted by the flashblocks payload builder.
//!
//! When [`crate::BuilderConfig::build_event_tx`] is set, the payload builder emits one
//! [`BuildEvent::IterationStart`] per payload job, one [`BuildEvent::TxExecuted`] per
//! committed transaction (with its receipt logs), and a terminal
//! [`BuildEvent::IterationComplete`] or [`BuildEvent::IterationAborted`]. Emission is
//! non-blocking (`try_send`); a full channel drops the event with a warning and never
//! stalls block building. [`BuildEventEmitter`] wraps the optional sender and performs
//! the emission.

use std::sync::atomic::{AtomicBool, Ordering};

use alloy_primitives::{Address, B256, Log};
use reth_payload_builder::PayloadId;
use tokio::sync::mpsc::{self, error::TrySendError};
use tracing::warn;

/// Sender half for the optional build-event stream.
pub type BuildEventSender = mpsc::Sender<BuildEvent>;

/// Lifecycle events for one payload build (one block-building iteration).
#[derive(Debug, Clone)]
pub enum BuildEvent {
    /// A payload build started.
    IterationStart {
        /// Identifier of the payload job; correlates all events of one iteration.
        payload_id: PayloadId,
        /// Number of the block being built.
        block_number: u64,
        /// Timestamp of the block being built.
        timestamp: u64,
        /// Base fee of the block being built.
        base_fee: u64,
    },
    /// A transaction was committed to the in-progress block.
    TxExecuted {
        /// Identifier of the payload job.
        payload_id: PayloadId,
        /// Hash of the committed transaction.
        tx_hash: B256,
        /// Transaction sender.
        from: Address,
        /// Transaction recipient, `None` for contract creation.
        to: Option<Address>,
        /// Receipt logs of the committed transaction.
        logs: Vec<Log>,
        /// Gas used by the transaction.
        gas_used: u64,
        /// Whether execution succeeded (receipt status).
        success: bool,
    },
    /// The payload was finalized (block sealed).
    IterationComplete {
        /// Identifier of the payload job.
        payload_id: PayloadId,
    },
    /// The payload build exited with an error before finalizing.
    IterationAborted {
        /// Identifier of the payload job.
        payload_id: PayloadId,
    },
}

/// Emits [`BuildEvent`]s to an optional consumer without ever blocking the payload builder.
///
/// Holds the optional channel and a one-shot latch so a closed consumer is reported once,
/// not per event. Construct one per builder and share it via `Arc`.
#[derive(Debug)]
pub struct BuildEventEmitter {
    sender: Option<BuildEventSender>,
    closed_warned: AtomicBool,
}

impl BuildEventEmitter {
    /// Creates an emitter; `None` makes every [`Self::emit`] a no-op.
    pub const fn new(sender: Option<BuildEventSender>) -> Self {
        Self { sender, closed_warned: AtomicBool::new(false) }
    }

    /// Whether a consumer is attached. Emit sites can skip building event payloads when false.
    pub const fn is_enabled(&self) -> bool {
        self.sender.is_some()
    }

    /// Emits a build event without blocking; drops the event if the channel is full
    /// or closed. No-op if no consumer is attached.
    ///
    /// A full channel warns on every drop (genuine backpressure). A closed channel
    /// (consumer gone) warns only once per emitter to avoid flooding logs for the
    /// remaining lifetime of the builder.
    pub fn emit(&self, event: BuildEvent) {
        let Some(sender) = &self.sender else { return };
        match sender.try_send(event) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                warn!("build event dropped: channel full");
            }
            Err(TrySendError::Closed(_)) => {
                if !self.closed_warned.swap(true, Ordering::Relaxed) {
                    warn!(
                        "build event channel closed, consumer gone — suppressing further warnings"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_event() -> BuildEvent {
        BuildEvent::IterationComplete { payload_id: PayloadId::new([0; 8]) }
    }

    #[test]
    fn emit_does_not_panic_on_full_channel() {
        let (tx, _rx) = mpsc::channel(1);
        tx.try_send(sample_event()).expect("first send fills capacity");
        let emitter = BuildEventEmitter::new(Some(tx));

        emitter.emit(sample_event());
    }

    #[test]
    fn emit_does_not_panic_on_closed_channel() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let emitter = BuildEventEmitter::new(Some(tx));

        emitter.emit(sample_event());
        emitter.emit(sample_event());
    }
}
