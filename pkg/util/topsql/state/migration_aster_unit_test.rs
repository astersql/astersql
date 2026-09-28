// Copyright 2026 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// TopSQL 状态模块的迁移期单元测试。
//
// 对齐 Go：TopSQL / TopRU 开关、TopProfiling 组合语义、
// RU 上报条目间隔校验与默认常量。

use crate::test_util::{lock_global_state, reset_global_state};
use topsql_state::*;

#[test]
/// 验证 Enable/Disable TopSQL 与 TopRU 的开关组合及 TopProfiling 派生语义对齐 Go。
fn migration_top_sql_and_top_ru_enablement_match_go() {
    let _guard = lock_global_state();
    reset_global_state();
    assert!(!TopSQLEnabled());
    assert!(!TopRUEnabled());
    assert!(!TopProfilingEnabled());

    EnableTopSQL();
    assert!(TopSQLEnabled());
    assert!(TopProfilingEnabled());
    DisableTopSQL();

    EnableTopRU();
    EnableTopRU();
    assert!(TopRUEnabled());
    assert!(TopProfilingEnabled());
    DisableTopRU();
    assert!(TopRUEnabled());
    DisableTopRU();
    DisableTopRU();
    assert!(!TopRUEnabled());
    assert!(!TopProfilingEnabled());
}

#[test]
/// 验证 TopRU 条目间隔仅允许合法值，非法值报错且关闭 TopRU 后复位默认。
fn migration_top_ru_interval_validation_and_reset_match_go() {
    let _guard = lock_global_state();
    reset_global_state();

    SetTopRUItemInterval(30).unwrap();
    SetTopRUItemInterval(15).unwrap();
    assert_eq!(GetTopRUItemInterval(), 15);

    let error = SetTopRUItemInterval(99).unwrap_err();
    assert_eq!(error.to_string(), "invalid top ru item interval: 99");
    assert_eq!(GetTopRUItemInterval(), 15);

    SetTopRUItemInterval(0).unwrap();
    assert_eq!(GetTopRUItemInterval(), DefTiDBTopRUItemIntervalSeconds);

    EnableTopRU();
    SetTopRUItemInterval(30).unwrap();
    DisableTopRU();
    assert_eq!(GetTopRUItemInterval(), DefTiDBTopRUItemIntervalSeconds);
}

#[test]
/// 验证 TopSQL / TopRU 默认常量与 GlobalState 初始原子值对齐 Go。
fn migration_top_sql_defaults_match_go() {
    let _guard = lock_global_state();
    use std::sync::atomic::Ordering;

    assert_eq!(DefTiDBTopSQLEnable, false);
    assert_eq!(DefTiDBTopSQLPrecisionSeconds, 1);
    assert_eq!(DefTiDBTopSQLMaxTimeSeriesCount, 100);
    assert_eq!(DefTiDBTopSQLMaxMetaCount, 5000);
    assert_eq!(DefTiDBTopSQLReportIntervalSeconds, 60);
    assert_eq!(DefTiDBTopRUItemIntervalSeconds, 60);
    assert_eq!(GlobalState.PrecisionSeconds.load(Ordering::SeqCst), 1);
    assert_eq!(GlobalState.MaxStatementCount.load(Ordering::SeqCst), 100);
    assert_eq!(GlobalState.MaxCollect.load(Ordering::SeqCst), 5000);
}
