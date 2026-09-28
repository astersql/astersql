// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 会话事务内部辅助：断言级别、进入新事务前提交、按时间戳取快照。
//
// 对应 Go `pkg/sessiontxn/internal` 中与 KV 事务选项相关的公共逻辑。
// 通过窄化的 `SessionTxnContext` 边界，让真实会话与原生单测共用同一实现。

use crate::{kv, kvrpcpb, sessionctx, variable};
use std::time::Duration;

/// 与 Go `error` 对齐的会话事务错误别名。
pub type SessionTxnError = sessionctx::GoError;

/// The exact subset of `sessionctx.Context` used by the three helpers below.
///
/// Keeping this boundary narrow lets a real session and focused native tests
/// share the same implementation without weakening the Go behavior.
///
/// 下方三个辅助函数所需的 `sessionctx.Context` 最小子集。
/// 收窄边界使真实会话与聚焦单测可共用实现，且不削弱与 Go 的行为一致性。
pub trait SessionTxnContext {
    /// 取当前事务；`active=false` 表示不强制激活。
    fn txn(&mut self, active: bool) -> Result<&mut dyn kv::Transaction, SessionTxnError>;
    /// 提交当前事务。
    fn commit_txn(&mut self, ctx: &sessionctx::ExecutionContext) -> Result<(), SessionTxnError>;
    /// 事务作用域（如 global / 本地时区 scope）。
    fn txn_scope(&self) -> &str;
    /// 当前 InfoSchema（信息模式）元版本号。
    fn schema_meta_version(&self) -> i64;

    /// 按版本（时间戳）获取 KV 快照（Snapshot）。
    fn get_snapshot(&self, version: kv::Version) -> Box<dyn kv::Snapshot>;
    /// 是否处于受限 SQL（内部系统 SQL）上下文。
    fn in_restricted_sql(&self) -> bool;
    /// 请求来源类型字符串。
    fn request_source_type(&self) -> &str;
    /// 显式指定的请求来源类型。
    fn explicit_request_source_type(&self) -> &str;
    /// 基于负载的副本读阈值（毫秒等单位由上游约定）。
    fn load_based_replica_read_threshold(&self) -> Duration;
}

/// 将会话变量中的断言级别映射为 KV RPC 的 `AssertionLevel` 并写入事务选项。
/// 断言（Assertion）用于在写路径校验键值前置条件。
pub fn set_txn_assertion_level(txn: &mut dyn kv::Transaction, level: variable::AssertionLevel) {
    let level = match level {
        variable::AssertionLevel::AssertionLevelOff => kvrpcpb::AssertionLevel::Off,
        variable::AssertionLevel::AssertionLevelFast => kvrpcpb::AssertionLevel::Fast,
        variable::AssertionLevel::AssertionLevelStrict => kvrpcpb::AssertionLevel::Strict,
    };
    txn.SetOption(kv::AssertionLevel, Some(Box::new(level)));
}

/// 进入新事务前：若旧事务仍有效则先提交，并记录 schemaVersion / startTS / scope。
pub fn commit_before_enter_new_txn<C: SessionTxnContext + ?Sized>(
    ctx: &sessionctx::ExecutionContext,
    sctx: &mut C,
) -> Result<(), SessionTxnError> {
    // 不激活旧事务；仅在 Valid 时取 start_ts。
    let start_ts = {
        let txn = sctx.txn(false)?;
        txn.Valid().then(|| txn.StartTS())
    };

    if let Some(start_ts) = start_ts {
        let txn_scope = sctx.txn_scope().to_owned();
        sctx.commit_txn(ctx)?;
        log::info!(
            "Try to create a new txn inside a transaction auto commit; schemaVersion={}, txnStartTS={}, txnScope={}",
            sctx.schema_meta_version(),
            start_ts,
            txn_scope,
        );
    }
    Ok(())
}

/// 按时间戳构造快照，并按 Go 顺序传播非默认选项（拦截器、内部源、来源类型、副本读阈值）。
pub fn get_snapshot_with_ts<C: SessionTxnContext + ?Sized>(
    sctx: &C,
    ts: u64,
    interceptor: Option<Box<dyn kv::SnapshotInterceptor>>,
) -> Box<dyn kv::Snapshot> {
    let mut snapshot = sctx.get_snapshot(kv::Version { Ver: ts });
    if let Some(interceptor) = interceptor {
        snapshot.SetOption(kv::SnapInterceptor, Some(Box::new(interceptor)));
    }
    if sctx.in_restricted_sql() {
        snapshot.SetOption(kv::RequestSourceInternal, Some(Box::new(true)));
    }
    if !sctx.request_source_type().is_empty() {
        snapshot.SetOption(
            kv::RequestSourceType,
            Some(Box::new(sctx.request_source_type().to_owned())),
        );
    }
    if !sctx.explicit_request_source_type().is_empty() {
        snapshot.SetOption(
            kv::ExplicitRequestSourceType,
            Some(Box::new(sctx.explicit_request_source_type().to_owned())),
        );
    }
    let threshold = sctx.load_based_replica_read_threshold();
    if !threshold.is_zero() {
        snapshot.SetOption(kv::LoadBasedReplicaReadThreshold, Some(Box::new(threshold)));
    }
    snapshot
}
