#![allow(missing_docs)]

use std::collections::HashSet;

use base_builder_core::{
    BuildEvent, BuilderConfig,
    test_utils::{TransactionBuilderExt, setup_test_instance_with_builder_config},
};

/// Verifies that a single payload job emits its build-event stream in order
/// (`IterationStart` first, `IterationComplete` last), includes a `TxExecuted` for the
/// submitted transfer, and that every event shares the job's `payload_id`.
#[tokio::test]
async fn build_events_stream_full_iteration() -> eyre::Result<()> {
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(1024);
    let mut config = BuilderConfig::for_tests();
    config.build_event_tx = Some(event_tx);

    let rbuilder = setup_test_instance_with_builder_config(config).await?;
    let driver = rbuilder.driver().await?;

    let tx = driver
        .create_transaction()
        .random_valid_transfer()
        .send()
        .await
        .expect("Failed to send transaction");

    driver.build_new_block_with_current_timestamp(None).await?;

    let mut events = Vec::new();
    while let Ok(event) = event_rx.try_recv() {
        events.push(event);
    }

    assert!(
        matches!(events.first(), Some(BuildEvent::IterationStart { .. })),
        "first event must be IterationStart, got {events:?}"
    );
    assert!(
        matches!(events.last(), Some(BuildEvent::IterationComplete { .. })),
        "last event must be IterationComplete, got {events:?}"
    );

    let executed: Vec<_> =
        events.iter().filter(|event| matches!(event, BuildEvent::TxExecuted { .. })).collect();
    assert!(!executed.is_empty(), "expected at least one TxExecuted event");
    let BuildEvent::TxExecuted { tx_hash, success, .. } = executed[0] else { unreachable!() };
    assert_eq!(*tx_hash, *tx.tx_hash(), "TxExecuted should report the submitted tx hash");
    assert!(*success, "submitted transfer should succeed");

    let payload_ids: HashSet<_> = events
        .iter()
        .map(|event| match event {
            BuildEvent::IterationStart { payload_id, .. }
            | BuildEvent::TxExecuted { payload_id, .. }
            | BuildEvent::IterationComplete { payload_id }
            | BuildEvent::IterationAborted { payload_id } => *payload_id,
        })
        .collect();
    assert_eq!(payload_ids.len(), 1, "all events must share one payload id, got {events:?}");

    Ok(())
}
