//! Intent-swap engine: consumes build events + submitted orders, solves, signs, inserts.

use alloy_consensus::transaction::Recovered;
use alloy_eips::Encodable2718;
use alloy_primitives::B256;
use alloy_signer_local::PrivateKeySigner;
use backrunner::{Backrunner, BackrunnerConfig, FusionOrder};
use base_builder_core::BuildEvent;
use base_common_consensus::BaseTransactionSigned;
use base_execution_txpool::BasePooledTransaction;
use builder_types::{BlockEnv, ExecutedTx};
use reth_transaction_pool::TransactionPool;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::{
    IntentSwapConfig,
    convert::{payload_uuid, raw_tx_to_signed, to_block_env, to_executed_tx},
};

/// State of the in-progress block build, accumulated from build events.
#[derive(Debug, Default)]
struct Iteration {
    uuid: Option<Uuid>,
    block: Option<BlockEnv>,
    txs: Vec<ExecutedTx>,
}

impl Iteration {
    /// Applies one build event to the iteration state.
    fn apply(&mut self, event: BuildEvent) {
        match event {
            BuildEvent::IterationStart { payload_id, block_number, timestamp, base_fee } => {
                debug!(block = block_number, "iteration started");
                *self = Self {
                    uuid: Some(payload_uuid(payload_id)),
                    block: Some(to_block_env(block_number, timestamp, base_fee)),
                    txs: Vec::new(),
                };
            }
            BuildEvent::TxExecuted { payload_id, .. } => {
                if self.uuid == Some(payload_uuid(payload_id))
                    && let Some(tx) = to_executed_tx(&event)
                {
                    self.txs.push(tx);
                }
            }
            BuildEvent::IterationComplete { .. } | BuildEvent::IterationAborted { .. } => {
                // cleared so committed/aborted txs don't overlay the next solve
                self.txs.clear();
            }
        }
    }
}

/// The intent-swap engine: one long-running task owning the backrunner solver.
pub struct IntentSwapEngine {
    backrunner: Backrunner,
    signer: PrivateKeySigner,
    chain_id: u64,
}

impl std::fmt::Debug for IntentSwapEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IntentSwapEngine").field("chain_id", &self.chain_id).finish_non_exhaustive()
    }
}

impl IntentSwapEngine {
    /// Builds the engine: parses the signer key and waits for the initial Tycho snapshot.
    pub async fn build(config: IntentSwapConfig) -> eyre::Result<Self> {
        let signer: PrivateKeySigner = config
            .eoa_private_key
            .parse()
            .map_err(|e| eyre::eyre!("invalid intent-swap EOA key: {e}"))?;
        let backrunner_config = BackrunnerConfig {
            chain: config.chain.clone(),
            tycho_url: config.tycho_url.clone(),
            rpc_url: config.rpc_url.clone(),
            tycho_api_key: config.tycho_api_key.clone(),
            protocols: config.protocols.clone(),
            min_tvl: config.min_tvl,
            ready_timeout: config.ready_timeout,
            chain_id: config.chain_id,
            resolver_address: config.resolver_address,
            slippage: config.slippage,
            orderbook_interval: config.orderbook_interval,
            verify_onchain_taking: config.verify_onchain_taking,
        };
        let backrunner = Backrunner::build(backrunner_config).await?;
        info!("intent-swap engine ready");
        Ok(Self { backrunner, signer, chain_id: config.chain_id })
    }

    /// Runs the engine loop until both channels close.
    pub async fn run<P>(
        self,
        mut events: mpsc::Receiver<BuildEvent>,
        mut orders: mpsc::Receiver<FusionOrder>,
        pool: P,
        initial_nonce: u64,
    ) where
        P: TransactionPool<Transaction = BasePooledTransaction> + Send + Sync + 'static,
    {
        let mut iteration = Iteration::default();
        let mut events_closed = false;
        let mut orders_closed = false;
        while !(events_closed && orders_closed) {
            tokio::select! {
                event = events.recv(), if !events_closed => {
                    match event {
                        Some(event) => {
                            self.log_settlement_inclusion(&iteration, &event);
                            iteration.apply(event);
                        }
                        None => {
                            debug!(channel = "events", "channel closed");
                            events_closed = true;
                        }
                    }
                }
                order = orders.recv(), if !orders_closed => {
                    match order {
                        Some(order) => {
                            self.solve_and_insert(&iteration, order, &pool, initial_nonce).await;
                        }
                        None => {
                            debug!(channel = "orders", "channel closed");
                            orders_closed = true;
                        }
                    }
                }
            }
        }
        info!("intent-swap engine shutting down");
    }

    /// Logs a `TxExecuted` event whose sender is this engine's signer: a settlement
    /// this engine submitted has been included in the in-progress block.
    fn log_settlement_inclusion(&self, iteration: &Iteration, event: &BuildEvent) {
        let BuildEvent::TxExecuted { from, tx_hash, .. } = event else {
            return;
        };
        if *from != self.signer.address() {
            return;
        }
        let block = iteration.block.as_ref().map(|b| b.block_number);
        info!(tx_hash = %tx_hash, block = ?block, "settlement included");
    }

    /// Solves one order against the current iteration state and inserts settlements.
    async fn solve_and_insert<P>(
        &self,
        iteration: &Iteration,
        order: FusionOrder,
        pool: &P,
        initial_nonce: u64,
    ) where
        P: TransactionPool<Transaction = BasePooledTransaction> + Send + Sync + 'static,
    {
        let order_hash = order.order_id.clone();
        let (Some(uuid), Some(block)) = (iteration.uuid, iteration.block.clone()) else {
            warn!(order_hash = %order_hash, "no build iteration observed yet, dropping order");
            return;
        };
        let candidate = self
            .backrunner
            .solve_orders(uuid, block, iteration.txs.clone(), std::slice::from_ref(&order))
            .await;
        let Some(candidate) = candidate else {
            info!(order_hash = %order_hash, "unfillable: no profitable route");
            return;
        };
        // pool holds this signer's next nonce; chain locally within the candidate
        let mut nonce = pool
            .get_highest_transaction_by_sender(self.signer.address())
            .map_or(initial_nonce, |tx| tx.nonce() + 1);
        for backrun_tx in &candidate.txs {
            let signed = raw_tx_to_signed(&backrun_tx.tx, self.chain_id, nonce, &self.signer);
            let signed = match signed {
                Ok(signed) => signed,
                Err(e) => {
                    error!(order_hash = %order_hash, error = %e, "failed to sign settlement");
                    continue;
                }
            };
            match Self::insert(pool, signed).await {
                Ok(tx_hash) => {
                    nonce += 1;
                    info!(
                        order_hash = %order_hash,
                        tx_hash = %tx_hash,
                        profit_wei = %backrun_tx.expected_profit_wei,
                        block = candidate.block_number,
                        "settlement queued",
                    );
                }
                Err(e) => {
                    error!(order_hash = %order_hash, error = %e, "pool rejected settlement");
                }
            }
        }
    }

    /// Inserts a signed settlement into the pool (same path as
    /// `base_insertValidatedTransaction`).
    async fn insert<P>(pool: &P, tx: Recovered<BaseTransactionSigned>) -> eyre::Result<B256>
    where
        P: TransactionPool<Transaction = BasePooledTransaction> + Send + Sync + 'static,
    {
        let tx_hash = *tx.hash();
        let encoded_length = tx.encoded_2718().len();
        let pool_tx = BasePooledTransaction::new(tx, encoded_length);
        pool.add_external_transaction(pool_tx)
            .await
            .map_err(|e| eyre::eyre!("pool insert failed: {e}"))?;
        Ok(tx_hash)
    }
}

#[cfg(test)]
mod tests {
    use alloy_primitives::Address;
    use reth_payload_builder::PayloadId;

    use super::*;

    fn start_event(payload_id: [u8; 8], block_number: u64) -> BuildEvent {
        BuildEvent::IterationStart {
            payload_id: PayloadId::new(payload_id),
            block_number,
            timestamp: 1_000,
            base_fee: 7,
        }
    }

    fn tx_event(payload_id: [u8; 8]) -> BuildEvent {
        BuildEvent::TxExecuted {
            payload_id: PayloadId::new(payload_id),
            tx_hash: B256::repeat_byte(0xaa),
            from: Address::repeat_byte(0x11),
            to: Some(Address::repeat_byte(0x22)),
            logs: Vec::new(),
            gas_used: 21_000,
            success: true,
        }
    }

    #[test]
    fn iteration_start_replaces_state() {
        let mut iteration = Iteration::default();
        iteration.txs.push(to_executed_tx(&tx_event([1; 8])).expect("tx event"));

        iteration.apply(start_event([2; 8], 42));

        assert_eq!(iteration.uuid, Some(payload_uuid(PayloadId::new([2; 8]))));
        assert_eq!(iteration.block.as_ref().map(|b| b.block_number), Some(42));
        assert!(iteration.txs.is_empty());
    }

    #[test]
    fn tx_executed_accumulates_for_matching_payload_id() {
        let mut iteration = Iteration::default();
        iteration.apply(start_event([3; 8], 1));

        iteration.apply(tx_event([3; 8]));
        iteration.apply(tx_event([3; 8]));

        assert_eq!(iteration.txs.len(), 2);
    }

    #[test]
    fn tx_executed_with_stale_payload_id_is_dropped() {
        let mut iteration = Iteration::default();
        iteration.apply(start_event([4; 8], 1));

        iteration.apply(tx_event([5; 8]));

        assert!(iteration.txs.is_empty());
    }

    #[test]
    fn iteration_complete_clears_txs_but_keeps_uuid_and_block() {
        let mut iteration = Iteration::default();
        iteration.apply(start_event([6; 8], 9));
        iteration.apply(tx_event([6; 8]));

        iteration.apply(BuildEvent::IterationComplete { payload_id: PayloadId::new([6; 8]) });

        assert!(iteration.txs.is_empty());
        assert_eq!(iteration.uuid, Some(payload_uuid(PayloadId::new([6; 8]))));
        assert_eq!(iteration.block.as_ref().map(|b| b.block_number), Some(9));
    }

    #[test]
    fn iteration_aborted_clears_txs_but_keeps_uuid_and_block() {
        let mut iteration = Iteration::default();
        iteration.apply(start_event([7; 8], 3));
        iteration.apply(tx_event([7; 8]));

        iteration.apply(BuildEvent::IterationAborted { payload_id: PayloadId::new([7; 8]) });

        assert!(iteration.txs.is_empty());
        assert_eq!(iteration.uuid, Some(payload_uuid(PayloadId::new([7; 8]))));
        assert_eq!(iteration.block.as_ref().map(|b| b.block_number), Some(3));
    }
}
