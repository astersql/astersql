// Copyright 2026 AsterSQL.

use serial_test::serial;

use crate::{Context, GetOwnerOpValue, NewMockManager, OpType};

#[tokio::test]
#[serial]
async fn mock_owner_op_value_is_process_global_like_go() {
    let ctx = Context::new();
    let first = NewMockManager(ctx.child_token(), "first", None, "/owner/first");
    first
        .SetOwnerOpValue(&ctx, OpType::OpSyncUpgradingState)
        .await
        .unwrap();
    assert_eq!(
        GetOwnerOpValue(&ctx, None, "/owner/other").await.unwrap(),
        OpType::OpSyncUpgradingState,
    );

    let second = NewMockManager(ctx.child_token(), "second", None, "/owner/second");
    assert_eq!(
        GetOwnerOpValue(&ctx, None, "/owner/first").await.unwrap(),
        OpType::OpNone,
    );

    first.Close().await;
    second.Close().await;
}
