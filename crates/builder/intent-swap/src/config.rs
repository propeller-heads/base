//! Configuration for the intent-swap extension.

use std::time::Duration;

use alloy_primitives::Address;

/// Configuration for the intent-swap engine and RPC.
#[derive(Clone)]
pub struct IntentSwapConfig {
    /// Tycho WebSocket host (e.g. `"tycho-beta.propellerheads.xyz"`).
    pub tycho_url: String,
    /// Tycho API key.
    pub tycho_api_key: Option<String>,
    /// JSON-RPC URL of the chain, used by the engine for on-chain order queries.
    pub rpc_url: String,
    /// Tycho protocol slugs to subscribe to.
    pub protocols: Vec<String>,
    /// Fusion resolver contract address settlements are routed through.
    pub resolver_address: Address,
    /// Chain slug for Tycho market data (e.g. `"base"`).
    pub chain: String,
    /// 1inch Fusion chain id (Base = 8453).
    pub chain_id: u64,
    /// Minimum pool TVL filter for Tycho subscriptions.
    pub min_tvl: f64,
    /// Route slippage tolerance (e.g. 0.005 = 0.5%).
    pub slippage: f64,
    /// EOA private key (0x-hex) used to sign settlement transactions.
    pub eoa_private_key: String,
    /// Capacity of the submitted-order queue.
    pub order_queue_capacity: usize,
    /// How long to wait for the initial Tycho market snapshot.
    pub ready_timeout: Duration,
    /// Interval for the engine's background orderbook poll (kept high; unused for solving).
    pub orderbook_interval: Duration,
    /// Enables the LOP on-chain preflight (`getTakingAmount` via `eth_call`) that rejects
    /// unfillable or unauthentic orders before the operator signs and pays gas to submit a
    /// settlement. `intent_submitOrder` performs no maker-signature authentication itself
    /// (deferred to the on-chain LOP by design), so this is the only check that runs before
    /// spending operator funds. Costs one extra RPC call per candidate; off by default to
    /// preserve existing behavior.
    pub verify_onchain_taking: bool,
}

impl std::fmt::Debug for IntentSwapConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IntentSwapConfig")
            .field("tycho_url", &self.tycho_url)
            .field("tycho_api_key", &self.tycho_api_key)
            .field("rpc_url", &self.rpc_url)
            .field("protocols", &self.protocols)
            .field("resolver_address", &self.resolver_address)
            .field("chain", &self.chain)
            .field("chain_id", &self.chain_id)
            .field("min_tvl", &self.min_tvl)
            .field("slippage", &self.slippage)
            .field("eoa_private_key", &"<redacted>")
            .field("order_queue_capacity", &self.order_queue_capacity)
            .field("ready_timeout", &self.ready_timeout)
            .field("orderbook_interval", &self.orderbook_interval)
            .field("verify_onchain_taking", &self.verify_onchain_taking)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_eoa_private_key() {
        let config = IntentSwapConfig {
            tycho_url: "tycho-beta.propellerheads.xyz".to_string(),
            tycho_api_key: None,
            rpc_url: "http://localhost:8545".to_string(),
            protocols: vec!["uniswap_v2".to_string()],
            resolver_address: Address::ZERO,
            chain: "base".to_string(),
            chain_id: 8453,
            min_tvl: 10.0,
            slippage: 0.005,
            eoa_private_key: "0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
                .to_string(),
            order_queue_capacity: 64,
            ready_timeout: Duration::from_secs(30),
            orderbook_interval: Duration::from_secs(60),
            verify_onchain_taking: false,
        };

        let debug_output = format!("{config:?}");

        assert!(!debug_output.contains("deadbeef"));
        assert!(debug_output.contains("redacted"));
    }
}
