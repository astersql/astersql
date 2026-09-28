// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 会话上下文迁移期单元测试：对照 Go 的 BasicCtxType 字符串与快照读校验转发。
//
// 覆盖：Ctx 类型枚举/未知值的 `String()` 表示；`ValidateSnapshotReadTS` 向
// Oracle 转发全部参数；Oracle 错误原样上抛。

use std::sync::Mutex;

use astersql_sessionctx::{
    BasicCtxType, ExecutionContext, GoError, Initing, LastExecuteDDL, OracleOption, QueryString,
    SnapshotReadOracle, SnapshotReadStorage, ValidateSnapshotReadTS,
};

/// 校验 BasicCtxType 各取值及未知值的字符串表示与 Go 表一致。
#[test]
fn basic_ctx_type_strings_match_go_table() {
    let cases = [
        (QueryString, "query_string"),
        (Initing, "initing"),
        (LastExecuteDDL, "last_execute_ddl"),
        (BasicCtxType::new(9), "unknown"),
    ];

    for (key, expected) in cases {
        assert_eq!(key.String(), expected);
        assert_eq!(key.to_string(), expected);
    }
}

/// 记录一次 `ValidateReadTS` 调用时传入的全部参数快照。
#[derive(Clone, Debug, Eq, PartialEq)]
struct ValidationCall {
    /// 快照读时间戳（start_ts / read_ts）。
    read_ts: u64,
    /// 是否为 stale read（读历史快照）。
    is_stale_read: bool,
    /// 事务作用域（TxnScope，如 global）。
    txn_scope: String,
    /// 调用时执行上下文是否已取消。
    context_cancelled: bool,
}

/// 可记录调用参数并可注入失败的假 Oracle，用于断言转发与错误透传。
struct RecordingOracle {
    calls: Mutex<Vec<ValidationCall>>,
    fail: bool,
}

impl SnapshotReadOracle for RecordingOracle {
    fn ValidateReadTS(
        &self,
        ctx: &ExecutionContext,
        read_ts: u64,
        is_stale_read: bool,
        option: &OracleOption<'_>,
    ) -> Result<(), GoError> {
        // 先记录调用参数，再按 fail 标志决定是否返回错误。
        self.calls
            .lock()
            .expect("calls mutex poisoned")
            .push(ValidationCall {
                read_ts,
                is_stale_read,
                txn_scope: option.TxnScope.to_owned(),
                context_cancelled: ctx.is_cancelled(),
            });
        if self.fail {
            return Err("read timestamp is in the future".into());
        }
        Ok(())
    }
}

/// 包装 RecordingOracle，实现 SnapshotReadStorage 以便走完整校验路径。
struct RecordingStorage {
    oracle: RecordingOracle,
}

impl SnapshotReadStorage for RecordingStorage {
    type Oracle = RecordingOracle;

    fn GetOracle(&self) -> &Self::Oracle {
        &self.oracle
    }
}

/// 校验 ValidateSnapshotReadTS 将 read_ts、stale、TxnScope、取消状态原样转发给 Oracle。
#[test]
fn snapshot_validation_forwards_all_go_arguments() {
    let ctx = ExecutionContext::new();
    ctx.cancel();
    let store = RecordingStorage {
        oracle: RecordingOracle {
            calls: Mutex::new(Vec::new()),
            fail: false,
        },
    };

    ValidateSnapshotReadTS(&ctx, &store, 4_294_967_299, true).unwrap();

    assert_eq!(
        *store.oracle.calls.lock().expect("calls mutex poisoned"),
        vec![ValidationCall {
            read_ts: 4_294_967_299,
            is_stale_read: true,
            txn_scope: "global".to_owned(),
            context_cancelled: true,
        }]
    );
}

/// 校验 Oracle 返回的错误消息不被包装改写，且仍会记录一次调用。
#[test]
fn snapshot_validation_returns_oracle_error_unchanged() {
    let ctx = ExecutionContext::new();
    let store = RecordingStorage {
        oracle: RecordingOracle {
            calls: Mutex::new(Vec::new()),
            fail: true,
        },
    };

    let error = ValidateSnapshotReadTS(&ctx, &store, 99, false).unwrap_err();
    assert_eq!(error.to_string(), "read timestamp is in the future");
    assert_eq!(
        store
            .oracle
            .calls
            .lock()
            .expect("calls mutex poisoned")
            .len(),
        1
    );
}
