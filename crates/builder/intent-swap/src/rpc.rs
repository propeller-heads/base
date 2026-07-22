//! `intent_` RPC namespace: signed Fusion order ingress.

use std::time::{SystemTime, UNIX_EPOCH};

use backrunner::FusionOrder;
use jsonrpsee::{
    core::RpcResult,
    proc_macros::rpc,
    types::{ErrorObject, ErrorObjectOwned},
};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, mpsc::error::TrySendError};
use tracing::info;

/// JSON-RPC error code: signature is not a 65-byte 0x-hex string.
pub const INVALID_SIGNATURE_CODE: i32 = -33001;
/// JSON-RPC error code: the order's Dutch auction window has ended.
pub const ORDER_EXPIRED_CODE: i32 = -33002;
/// JSON-RPC error code: structurally invalid order (zero amount, empty id).
pub const INVALID_ORDER_CODE: i32 = -33003;
/// JSON-RPC error code: intake queue is full; retry later.
pub const QUEUE_FULL_CODE: i32 = -33004;
/// JSON-RPC error code: the engine's order channel has been dropped.
pub const ENGINE_UNAVAILABLE_CODE: i32 = -33005;

/// Response to a successful order submission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitOrderResponse {
    /// Hash/id of the accepted order.
    pub order_hash: String,
    /// Always `"accepted"`; solving happens asynchronously.
    pub status: String,
}

/// RPC interface for submitting signed Fusion orders to the intent-swap engine.
#[rpc(server, namespace = "intent")]
pub trait IntentApi {
    /// Validates and enqueues a signed Fusion order for immediate solving.
    #[method(name = "submitOrder")]
    async fn submit_order(&self, order: FusionOrder) -> RpcResult<SubmitOrderResponse>;
}

/// Implementation of [`IntentApiServer`] that forwards accepted orders to the engine.
#[derive(Debug)]
pub struct IntentApiImpl {
    orders: mpsc::Sender<FusionOrder>,
}

impl IntentApiImpl {
    /// Creates a new API handing accepted orders to `orders`.
    pub const fn new(orders: mpsc::Sender<FusionOrder>) -> Self {
        Self { orders }
    }
}

/// Validates order shape and auction window; returns a structured RPC error on failure.
fn validate(order: &FusionOrder) -> Result<(), ErrorObjectOwned> {
    let sig = order.signature.strip_prefix("0x").unwrap_or(&order.signature);
    if sig.len() != 130 || alloy_primitives::hex::decode(sig).is_err() {
        return Err(ErrorObject::owned(
            INVALID_SIGNATURE_CODE,
            "signature must be 65 bytes of 0x-hex",
            None::<()>,
        ));
    }
    if order.order_id.is_empty() || order.making_amount.is_zero() {
        return Err(ErrorObject::owned(
            INVALID_ORDER_CODE,
            "order_id empty or making_amount zero",
            None::<()>,
        ));
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    if order.auction_start_time.saturating_add(order.auction_duration_secs) < now {
        return Err(ErrorObject::owned(ORDER_EXPIRED_CODE, "auction window has ended", None::<()>));
    }
    Ok(())
}

#[async_trait::async_trait]
impl IntentApiServer for IntentApiImpl {
    async fn submit_order(&self, order: FusionOrder) -> RpcResult<SubmitOrderResponse> {
        validate(&order)?;
        let order_hash = order.order_id.clone();
        self.orders.try_send(order).map_err(|e| match e {
            TrySendError::Full(_) => {
                ErrorObject::owned(QUEUE_FULL_CODE, "order queue full, retry later", None::<()>)
            }
            TrySendError::Closed(_) => {
                ErrorObject::owned(ENGINE_UNAVAILABLE_CODE, "engine unavailable", None::<()>)
            }
        })?;
        info!(order_hash = %order_hash, "fusion order accepted");
        Ok(SubmitOrderResponse { order_hash, status: "accepted".to_owned() })
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;

    use super::*;

    fn valid_order() -> backrunner::FusionOrder {
        // Minimal structurally-valid order: 65-byte signature, active auction, non-zero amount.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_secs();
        serde_json::from_value(serde_json::json!({
            "order_id": "0xabc123",
            "from_token": "0x4200000000000000000000000000000000000006",
            "to_token": "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913",
            "making_amount": "0x0de0b6b3a7640000",
            "auction_start_amount": "0x0f4240",
            "auction_end_amount": "0x0e4e1c",
            "auction_duration_secs": 180,
            "auction_start_time": now,
            "points": [],
            "from_token_symbol": null,
            "to_token_symbol": null,
            "from_token_decimals": 18,
            "to_token_decimals": 6,
            "from_token_usd_rate": 0.0,
            "to_token_usd_rate": 0.0,
            "gas_bump_estimate": 0,
            "gas_price_estimate_mwei": 0,
            "init_rate_bump": 0,
            "total_fees_1e5": 0,
            "signature": format!("0x{}", "11".repeat(65)),
            "extension": "0x",
            "salt": "1",
            "maker_address": "0x0000000000000000000000000000000000000001",
            "receiver_address": "0x0000000000000000000000000000000000000001",
            "maker_traits": "0"
        }))
        .expect("valid order json")
    }

    #[tokio::test]
    async fn accepts_valid_order() {
        let (tx, mut rx) = mpsc::channel(4);
        let api = IntentApiImpl::new(tx);
        let resp = api.submit_order(valid_order()).await.expect("accepted");
        assert_eq!(resp.status, "accepted");
        assert_eq!(resp.order_hash, "0xabc123");
        assert!(rx.try_recv().is_ok());
    }

    #[tokio::test]
    async fn rejects_malformed_signature() {
        let (tx, _rx) = mpsc::channel(4);
        let api = IntentApiImpl::new(tx);
        let mut order = valid_order();
        order.signature = "0xdead".to_owned();
        let err = api.submit_order(order).await.expect_err("must reject");
        assert_eq!(err.code(), INVALID_SIGNATURE_CODE);
    }

    #[tokio::test]
    async fn rejects_expired_auction() {
        let (tx, _rx) = mpsc::channel(4);
        let api = IntentApiImpl::new(tx);
        let mut order = valid_order();
        order.auction_start_time = 1_000_000; // long past
        let err = api.submit_order(order).await.expect_err("must reject");
        assert_eq!(err.code(), ORDER_EXPIRED_CODE);
    }

    #[tokio::test]
    async fn rejects_zero_making_amount() {
        let (tx, _rx) = mpsc::channel(4);
        let api = IntentApiImpl::new(tx);
        let mut order = valid_order();
        order.making_amount = alloy_primitives::U256::ZERO;
        let err = api.submit_order(order).await.expect_err("must reject");
        assert_eq!(err.code(), INVALID_ORDER_CODE);
    }

    #[tokio::test]
    async fn rejects_when_queue_full() {
        let (tx, _rx) = mpsc::channel(1);
        let api = IntentApiImpl::new(tx);
        api.submit_order(valid_order()).await.expect("first accepted");
        let err = api.submit_order(valid_order()).await.expect_err("queue full");
        assert_eq!(err.code(), QUEUE_FULL_CODE);
    }

    #[tokio::test]
    async fn rejects_when_engine_unavailable() {
        let (tx, rx) = mpsc::channel(4);
        drop(rx);
        let api = IntentApiImpl::new(tx);
        let err = api.submit_order(valid_order()).await.expect_err("engine gone");
        assert_eq!(err.code(), ENGINE_UNAVAILABLE_CODE);
    }
}
