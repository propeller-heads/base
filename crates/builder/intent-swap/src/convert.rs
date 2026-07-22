//! Conversions between base-builder-core events, builder-types, and signed transactions.

use alloy_consensus::{
    SignableTransaction, TxEip1559, TxEnvelope,
    transaction::{Recovered, SignerRecoverable},
};
use alloy_primitives::TxKind;
use alloy_signer::SignerSync;
use alloy_signer_local::PrivateKeySigner;
use base_builder_core::BuildEvent;
use base_common_consensus::BaseTransactionSigned;
use builder_types::{BlockEnv, ExecutedTx, RawTx};
use reth_payload_builder::PayloadId;
use uuid::Uuid;

/// Derives a stable UUID from a payload id (payload ids are 8 bytes; zero-padded to 16).
pub fn payload_uuid(payload_id: PayloadId) -> Uuid {
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(payload_id.0.as_slice());
    Uuid::from_bytes(bytes)
}

/// Converts an `IterationStart` event's fields into a builder-types [`BlockEnv`].
pub const fn to_block_env(block_number: u64, timestamp: u64, base_fee: u64) -> BlockEnv {
    BlockEnv { block_number, block_timestamp: timestamp, base_fee_per_gas: base_fee }
}

/// Converts a `TxExecuted` build event into a builder-types [`ExecutedTx`].
///
/// Returns `None` for non-`TxExecuted` variants.
pub fn to_executed_tx(event: &BuildEvent) -> Option<ExecutedTx> {
    let BuildEvent::TxExecuted { tx_hash, from, to, logs, gas_used, success, .. } = event else {
        return None;
    };
    Some(ExecutedTx {
        tx_hash: *tx_hash,
        from: *from,
        to: *to,
        logs: logs.clone(),
        gas_used: *gas_used,
        status: *success,
    })
}

/// Builds and signs an EIP-1559 transaction from a settlement [`RawTx`], returning it as
/// a base-chain [`BaseTransactionSigned`] with the recovered signer attached.
pub fn raw_tx_to_signed(
    raw: &RawTx,
    chain_id: u64,
    nonce: u64,
    signer: &PrivateKeySigner,
) -> eyre::Result<Recovered<BaseTransactionSigned>> {
    let tx = TxEip1559 {
        chain_id,
        nonce,
        gas_limit: raw.gas_limit,
        max_fee_per_gas: raw.max_fee_per_gas,
        max_priority_fee_per_gas: raw.max_priority_fee_per_gas,
        to: raw.to.map_or(TxKind::Create, TxKind::Call),
        value: raw.value,
        access_list: Default::default(),
        input: raw.data.clone(),
    };
    let signature = signer.sign_hash_sync(&tx.signature_hash())?;
    let envelope = TxEnvelope::Eip1559(tx.into_signed(signature));
    let base_tx = BaseTransactionSigned::try_from(envelope)
        .map_err(|_| eyre::eyre!("EIP-1559 settlement rejected by base envelope conversion"))?;
    base_tx
        .try_into_recovered()
        .map_err(|e| eyre::eyre!("failed to recover settlement signer: {e}"))
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{Bytes, U256};
    use alloy_signer_local::PrivateKeySigner;
    use reth_payload_builder::PayloadId;

    use super::*;

    #[test]
    fn payload_uuid_is_deterministic_and_distinct() {
        let a = PayloadId::new([1, 2, 3, 4, 5, 6, 7, 8]);
        let b = PayloadId::new([1, 2, 3, 4, 5, 6, 7, 9]);
        assert_eq!(payload_uuid(a), payload_uuid(a));
        assert_ne!(payload_uuid(a), payload_uuid(b));
    }

    #[test]
    fn signed_raw_tx_roundtrips_with_correct_sender() {
        let signer = PrivateKeySigner::random();
        let raw = builder_types::RawTx {
            to: Some(alloy_primitives::Address::repeat_byte(0x42)),
            value: U256::ZERO,
            data: Bytes::from(vec![0xca, 0xfe]),
            gas_limit: 500_000,
            max_fee_per_gas: 2_000_000_000,
            max_priority_fee_per_gas: 100_000_000,
        };
        let recovered = raw_tx_to_signed(&raw, 8453, 7, &signer).expect("sign");
        assert_eq!(recovered.signer(), signer.address());
    }
}
