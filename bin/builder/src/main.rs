#![doc = include_str!("../README.md")]
#![doc(issue_tracker_base_url = "https://github.com/base/base/issues/")]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use std::sync::Arc;

use base_builder_cli::Args;
use base_builder_core::{BuilderApiExtension, FlashblocksServiceBuilder};
use base_builder_metering::MeteringStoreExtension;
use base_execution_cli::{Cli, StandardBaseRethNode};
use base_node_runner::BaseNodeRunner;
use base_observability_events::GlobalTransactionEventWriter;
use base_txpool_rpc::{TxPoolRpcConfig, TxPoolRpcExtension};

type BuilderCli = Cli<Args>;

#[global_allocator]
static ALLOC: reth_cli_util::allocator::Allocator = reth_cli_util::allocator::new_allocator();

fn main() {
    base_cli_utils::init_common!();
    base_reth_cli::init_reth!();
    base_reth_cli::init_snapshots!();

    let cli = base_cli_utils::parse_cli!(BuilderCli);

    cli.run(|builder, builder_args| async move {
        let rollup_args = builder_args.rollup_args.clone();
        let builder = StandardBaseRethNode::apply_initial_upgrade_signal_from_rollup_args(
            builder,
            &rollup_args,
        )
        .await?;

        let metering_provider: base_builder_core::SharedMeteringProvider =
            Arc::new(builder_args.build_metering_store());
        let transaction_events_enabled = builder_args.transaction_events.enabled;
        GlobalTransactionEventWriter::init(
            transaction_events_enabled.then(|| builder_args.transaction_events.writer_config()),
        )?;

        let intent_swap_args = builder_args.intent_swap.clone();
        let mut builder_config = builder_args
            .into_builder_config(Arc::clone(&metering_provider))
            .expect("Failed to convert rollup args to builder config");
        let da_config = builder_config.da_config.clone();
        let gas_limit_config = builder_config.gas_limit_config.clone();

        let intent_swap = if intent_swap_args.enabled {
            let (event_tx, event_rx) =
                tokio::sync::mpsc::channel(intent_swap_args.event_channel_capacity);
            builder_config.build_event_tx = Some(event_tx);
            let config = intent_swap_config(&intent_swap_args)?;
            Some((config, event_rx))
        } else {
            None
        };

        let mut runner = BaseNodeRunner::new(rollup_args.clone())
            .with_da_config(da_config)
            .with_gas_limit_config(gas_limit_config)
            .with_service_builder(FlashblocksServiceBuilder(builder_config));
        runner.install_ext::<MeteringStoreExtension>(metering_provider);
        runner.install_ext::<TxPoolRpcExtension>(TxPoolRpcConfig::default());
        runner.install_ext::<BuilderApiExtension>(());
        StandardBaseRethNode::install_upgrade_signal_runtime_extension(&mut runner, &rollup_args)?;
        if let Some(config_and_rx) = intent_swap {
            runner.install_ext::<base_intent_swap::IntentSwapExtension>(config_and_rx);
        }
        runner.add_started_callback(|| {
            base_cli_utils::register_version_metrics!();
            Ok(())
        });

        runner.run(builder).await
    })
    .unwrap();
}

/// Builds the intent-swap config from CLI args; errors on missing required values.
fn intent_swap_config(
    args: &base_builder_cli::IntentSwapArgs,
) -> eyre::Result<base_intent_swap::IntentSwapConfig> {
    Ok(base_intent_swap::IntentSwapConfig {
        tycho_url: args.tycho_url.clone(),
        tycho_api_key: args.tycho_api_key.clone(),
        rpc_url: args
            .rpc_url
            .clone()
            .ok_or_else(|| eyre::eyre!("--builder.intent-swap.rpc-url is required"))?,
        protocols: args.protocols.clone(),
        resolver_address: args
            .resolver_address
            .as_deref()
            .ok_or_else(|| eyre::eyre!("--builder.intent-swap.resolver-address is required"))?
            .parse()
            .map_err(|e| eyre::eyre!("invalid resolver address: {e}"))?,
        chain: args.chain.clone(),
        chain_id: args.chain_id,
        min_tvl: args.min_tvl,
        slippage: args.slippage,
        eoa_private_key: args
            .eoa_key
            .clone()
            .ok_or_else(|| eyre::eyre!("INTENT_SWAP_EOA_KEY is required"))?,
        order_queue_capacity: args.order_queue_capacity,
        ready_timeout: std::time::Duration::from_secs(600),
        orderbook_interval: std::time::Duration::from_secs(60),
        verify_onchain_taking: args.verify_onchain_taking,
    })
}
