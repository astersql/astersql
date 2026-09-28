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

// `sessiontxn/internal` 迁移期单元测试。
//
// 用 Mock 事务/快照/会话上下文验证：断言级别映射、进入新事务前提交、
// 按时间戳取快照时选项传播顺序及空值省略，与 Go 行为对齐。

use std::any::Any;
use std::cell::RefCell;
use std::io;
use std::rc::Rc;
use std::time::Duration;

use astersql_sessiontxn_internal::{
    SessionTxnContext, SessionTxnError, commit_before_enter_new_txn, get_snapshot_with_ts, kv,
    kvrpcpb, sessionctx, set_txn_assertion_level, variable,
};

/// 记录 Mock 上 `SetOption` 写入的类型化值，便于断言。
#[derive(Clone, Debug, PartialEq)]
enum RecordedValue {
    Assertion(kvrpcpb::AssertionLevel),
    Bool(bool),
    Text(String),
    Threshold(Duration),
    Interceptor,
}

/// 将 `(option, Any)` 解码为 `RecordedValue`；未知 option 直接 panic。
fn decode_option(option: i32, value: Option<Box<dyn Any>>) -> RecordedValue {
    let value = value.expect("the Go implementation always supplies these option values");
    match option {
        kv::AssertionLevel => RecordedValue::Assertion(
            *value
                .downcast::<kvrpcpb::AssertionLevel>()
                .expect("assertion option must carry kvrpcpb::AssertionLevel"),
        ),
        kv::RequestSourceInternal => RecordedValue::Bool(
            *value
                .downcast::<bool>()
                .expect("internal-source option must carry bool"),
        ),
        kv::RequestSourceType | kv::ExplicitRequestSourceType => RecordedValue::Text(
            *value
                .downcast::<String>()
                .expect("request-source option must carry String"),
        ),
        kv::LoadBasedReplicaReadThreshold => RecordedValue::Threshold(
            *value
                .downcast::<Duration>()
                .expect("load threshold option must carry Duration"),
        ),
        kv::SnapInterceptor => RecordedValue::Interceptor,
        _ => panic!("unexpected option {option}"),
    }
}

/// 可配置 Valid/StartTS 并记录选项的 Mock 事务。
#[derive(Default)]
struct MockTransaction {
    valid: bool,
    start_ts: u64,
    options: Vec<(i32, RecordedValue)>,
}

impl kv::Transaction for MockTransaction {
    fn SetOption(&mut self, option: i32, value: Option<Box<dyn Any>>) {
        self.options.push((option, decode_option(option, value)));
    }

    fn Valid(&self) -> bool {
        self.valid
    }

    fn StartTS(&self) -> u64 {
        self.start_ts
    }
}

/// 通过共享 `RefCell` 记录快照选项的 Mock Snapshot。
struct MockSnapshot {
    options: Rc<RefCell<Vec<(i32, RecordedValue)>>>,
}

impl kv::Snapshot for MockSnapshot {
    fn SetOption(&mut self, option: i32, value: Option<Box<dyn Any>>) {
        self.options
            .borrow_mut()
            .push((option, decode_option(option, value)));
    }
}

/// 空拦截器桩，仅占位 `SnapInterceptor` 选项。
struct MockInterceptor;
impl kv::SnapshotInterceptor for MockInterceptor {}

/// 实现 `SessionTxnContext` 的测试双：可注入查找/提交失败与各类会话字段。
struct MockContext {
    txn: MockTransaction,
    txn_lookup_fails: bool,
    commit_fails: bool,
    commit_calls: usize,
    txn_scope: String,
    schema_meta_version: i64,
    in_restricted_sql: bool,
    request_source_type: String,
    explicit_request_source_type: String,
    load_based_replica_read_threshold: Duration,
    snapshot_version: Rc<RefCell<Option<kv::Version>>>,
    snapshot_options: Rc<RefCell<Vec<(i32, RecordedValue)>>>,
}

impl Default for MockContext {
    fn default() -> Self {
        Self {
            txn: MockTransaction::default(),
            txn_lookup_fails: false,
            commit_fails: false,
            commit_calls: 0,
            txn_scope: "global".to_owned(),
            schema_meta_version: 42,
            in_restricted_sql: false,
            request_source_type: String::new(),
            explicit_request_source_type: String::new(),
            load_based_replica_read_threshold: Duration::ZERO,
            snapshot_version: Rc::new(RefCell::new(None)),
            snapshot_options: Rc::new(RefCell::new(Vec::new())),
        }
    }
}

/// 构造带固定消息的 `SessionTxnError`（包装 `io::Error`）。
fn test_error(message: &'static str) -> SessionTxnError {
    Box::new(io::Error::other(message))
}

impl SessionTxnContext for MockContext {
    fn txn(&mut self, active: bool) -> Result<&mut dyn kv::Transaction, SessionTxnError> {
        // CommitBeforeEnterNewTxn 不得激活旧事务。
        assert!(
            !active,
            "CommitBeforeEnterNewTxn must not activate the old txn"
        );
        if self.txn_lookup_fails {
            Err(test_error("txn lookup failed"))
        } else {
            Ok(&mut self.txn)
        }
    }

    fn commit_txn(&mut self, _ctx: &sessionctx::ExecutionContext) -> Result<(), SessionTxnError> {
        self.commit_calls += 1;
        if self.commit_fails {
            Err(test_error("commit failed"))
        } else {
            Ok(())
        }
    }

    fn txn_scope(&self) -> &str {
        &self.txn_scope
    }

    fn schema_meta_version(&self) -> i64 {
        self.schema_meta_version
    }

    fn get_snapshot(&self, version: kv::Version) -> Box<dyn kv::Snapshot> {
        self.snapshot_version.replace(Some(version));
        Box::new(MockSnapshot {
            options: Rc::clone(&self.snapshot_options),
        })
    }

    fn in_restricted_sql(&self) -> bool {
        self.in_restricted_sql
    }

    fn request_source_type(&self) -> &str {
        &self.request_source_type
    }

    fn explicit_request_source_type(&self) -> &str {
        &self.explicit_request_source_type
    }

    fn load_based_replica_read_threshold(&self) -> Duration {
        self.load_based_replica_read_threshold
    }
}

/// Off/Fast/Strict 三级断言均映射到对应 `kvrpcpb::AssertionLevel`。
#[test]
fn set_txn_assertion_level_maps_all_go_levels() {
    let cases = [
        (
            variable::AssertionLevel::AssertionLevelOff,
            kvrpcpb::AssertionLevel::Off,
        ),
        (
            variable::AssertionLevel::AssertionLevelFast,
            kvrpcpb::AssertionLevel::Fast,
        ),
        (
            variable::AssertionLevel::AssertionLevelStrict,
            kvrpcpb::AssertionLevel::Strict,
        ),
    ];

    for (level, expected) in cases {
        let mut txn = MockTransaction::default();
        set_txn_assertion_level(&mut txn, level);
        assert_eq!(
            txn.options,
            vec![(kv::AssertionLevel, RecordedValue::Assertion(expected))]
        );
    }
}

/// 覆盖无效事务跳过提交、有效事务提交、查找失败与提交失败四条路径。
#[test]
fn commit_before_enter_new_txn_matches_validity_and_error_flow() {
    let ctx = sessionctx::ExecutionContext::new();

    let mut inactive = MockContext::default();
    commit_before_enter_new_txn(&ctx, &mut inactive).unwrap();
    assert_eq!(inactive.commit_calls, 0);

    let mut active = MockContext::default();
    active.txn.valid = true;
    active.txn.start_ts = 1234;
    active.txn_scope = "zone-a".to_owned();
    commit_before_enter_new_txn(&ctx, &mut active).unwrap();
    assert_eq!(active.commit_calls, 1);

    let mut lookup_error = MockContext {
        txn_lookup_fails: true,
        ..MockContext::default()
    };
    assert_eq!(
        commit_before_enter_new_txn(&ctx, &mut lookup_error)
            .unwrap_err()
            .to_string(),
        "txn lookup failed"
    );
    assert_eq!(lookup_error.commit_calls, 0);

    let mut commit_error = MockContext {
        txn: MockTransaction {
            valid: true,
            start_ts: 5678,
            options: Vec::new(),
        },
        commit_fails: true,
        ..MockContext::default()
    };
    assert_eq!(
        commit_before_enter_new_txn(&ctx, &mut commit_error)
            .unwrap_err()
            .to_string(),
        "commit failed"
    );
    assert_eq!(commit_error.commit_calls, 1);
}

/// 非默认选项按 Go 顺序全部写入快照。
#[test]
fn get_snapshot_with_ts_propagates_all_non_default_options_in_go_order() {
    let context = MockContext {
        in_restricted_sql: true,
        request_source_type: "internal-ddl".to_owned(),
        explicit_request_source_type: "backfill".to_owned(),
        load_based_replica_read_threshold: Duration::from_nanos(8192),
        ..MockContext::default()
    };

    let _snapshot = get_snapshot_with_ts(&context, 99, Some(Box::new(MockInterceptor)));

    assert_eq!(
        *context.snapshot_version.borrow(),
        Some(kv::Version { Ver: 99 })
    );
    assert_eq!(
        *context.snapshot_options.borrow(),
        vec![
            (kv::SnapInterceptor, RecordedValue::Interceptor),
            (kv::RequestSourceInternal, RecordedValue::Bool(true)),
            (
                kv::RequestSourceType,
                RecordedValue::Text("internal-ddl".to_owned())
            ),
            (
                kv::ExplicitRequestSourceType,
                RecordedValue::Text("backfill".to_owned())
            ),
            (
                kv::LoadBasedReplicaReadThreshold,
                RecordedValue::Threshold(Duration::from_nanos(8192))
            ),
        ]
    );
}

/// nil 拦截器、空来源字符串与零阈值时不写入任何选项。
#[test]
fn get_snapshot_with_ts_omits_nil_empty_and_zero_options() {
    let context = MockContext::default();

    let _snapshot = get_snapshot_with_ts(&context, 0, None);

    assert_eq!(
        *context.snapshot_version.borrow(),
        Some(kv::Version { Ver: 0 })
    );
    assert!(context.snapshot_options.borrow().is_empty());
}
