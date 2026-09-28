// Copyright 2026 AsterSQL.

//! 与 Go `br/pkg/operation` 公开契约的 parity：Hint 字段与 LockMeta。

use crate::{LockResourceMigrationRead, LockResourceType, NewContext};

#[test]
fn go_rust_public_contract_matches() {
    // 新建 backup 操作上下文，初始无 hint。
    let mut ctx = NewContext("backup").unwrap();
    assert!(!ctx.OperationID.is_empty());
    assert!(ctx.HintFields().is_empty());

    // 同 key 覆盖；空 value 删除；空 key 忽略。
    ctx.SetHintField("cluster", "tidb-1");
    assert_eq!(ctx.HintFields().len(), 1);
    assert_eq!(ctx.HintFields()[0].Value, "tidb-1");
    ctx.SetHintField("cluster", "tidb-2");
    assert_eq!(ctx.HintFields()[0].Value, "tidb-2");
    ctx.SetHintField("cluster", "");
    assert!(ctx.HintFields().is_empty());
    ctx.SetHintField("", "x");
    assert!(ctx.HintFields().is_empty());

    // LockMeta 绑定 OwnerID/类型，Hint 含 started_at 与 detail。
    let meta = ctx
        .LockMeta(LockResourceMigrationRead, "detail-msg")
        .unwrap();
    assert_eq!(meta.OwnerID, ctx.OperationID);
    assert_eq!(meta.LockType, "migration-read");
    assert!(meta.Hint.contains("operation_started_at="));
    assert!(meta.Hint.contains("detail="));

    // 空资源类型应失败。
    assert!(ctx.LockMeta(LockResourceType(""), "x").is_err());
}
