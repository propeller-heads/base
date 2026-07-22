//! Node extension registering the intent RPC and spawning the engine task.

use base_builder_core::BuildEvent;
use base_node_runner::{BaseNodeExtension, BaseRpcContext, FromExtensionConfig, NodeHooks};
use reth_provider::StateProviderFactory;
use tokio::sync::mpsc;
use tracing::{error, info};

use crate::{IntentApiImpl, IntentApiServer, IntentSwapConfig, IntentSwapEngine};

/// Extension that registers the `intent_` RPC namespace and spawns the intent-swap engine.
#[derive(Debug)]
pub struct IntentSwapExtension {
    config: IntentSwapConfig,
    event_rx: mpsc::Receiver<BuildEvent>,
}

impl FromExtensionConfig for IntentSwapExtension {
    type Config = (IntentSwapConfig, mpsc::Receiver<BuildEvent>);

    fn from_config((config, event_rx): Self::Config) -> Self {
        Self { config, event_rx }
    }
}

impl BaseNodeExtension for IntentSwapExtension {
    fn apply(self: Box<Self>, hooks: NodeHooks) -> NodeHooks {
        let Self { config, event_rx } = *self;
        let (order_tx, order_rx) = mpsc::channel(config.order_queue_capacity);

        let hooks = hooks.add_rpc_module(move |ctx: &mut BaseRpcContext<'_>| {
            let api = IntentApiImpl::new(order_tx);
            ctx.modules.merge_configured(api.into_rpc())?;
            info!("intent_submitOrder RPC enabled");
            Ok(())
        });

        hooks.add_node_started_hook(move |ctx| {
            let pool = ctx.pool().clone();
            let signer_address = config
                .eoa_private_key
                .parse::<alloy_signer_local::PrivateKeySigner>()
                .map(|s| s.address())
                .map_err(|e| eyre::eyre!("invalid intent-swap EOA key: {e}"))?;
            let initial_nonce =
                ctx.provider().latest()?.account_nonce(&signer_address)?.unwrap_or(0);
            ctx.task_executor.spawn_critical_task("intent-swap-engine", async move {
                match IntentSwapEngine::build(config).await {
                    Ok(engine) => engine.run(event_rx, order_rx, pool, initial_nonce).await,
                    Err(e) => {
                        error!(error = %e, "intent-swap engine failed to start");
                        panic!("intent-swap engine failed to start: {e}");
                    }
                }
            });
            info!("intent-swap engine task spawned");
            Ok(())
        })
    }
}
