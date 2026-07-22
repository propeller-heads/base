#![doc = include_str!("../README.md")]
#![doc(issue_tracker_base_url = "https://github.com/base/base/issues/")]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

mod config;
pub use config::IntentSwapConfig;

mod rpc;
pub use rpc::{
    ENGINE_UNAVAILABLE_CODE, INVALID_ORDER_CODE, INVALID_SIGNATURE_CODE, IntentApiImpl,
    IntentApiServer, ORDER_EXPIRED_CODE, QUEUE_FULL_CODE, SubmitOrderResponse,
};

mod convert;
pub use convert::{payload_uuid, raw_tx_to_signed, to_block_env, to_executed_tx};

mod engine;
pub use engine::IntentSwapEngine;
