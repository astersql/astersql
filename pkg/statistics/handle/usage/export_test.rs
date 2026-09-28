// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 统计使用量模块的测试导出/时长覆盖回归。
//
// Go 侧通过 export_test.go 修改包级 dumpStatsMaxDuration；Rust 侧将同一配置
// 收敛到 StatsUsageImpl 实例，避免并行测试共享可变全局状态。

use crate::{
    ColumnTimeInfo, DUMP_STATS_MAX_DURATION, Error, SchemaState, StatsUsageImpl, TableDelta,
    TableItemId, UsageStore,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

struct NoopStore;

impl UsageStore for NoopStore {
    fn stats_meta_count(&self, _table_id: i64) -> Result<Option<i64>, Error> {
        Ok(None)
    }

    fn update_delta(&self, _table_id: i64, _delta: TableDelta, _locked: bool) -> Result<(), Error> {
        Ok(())
    }

    fn save_column_usage(&self, _entries: &[crate::ColStatsUsageEntry]) -> Result<(), Error> {
        Ok(())
    }

    fn load_column_usage(&self) -> Result<HashMap<TableItemId, ColumnTimeInfo>, Error> {
        Ok(HashMap::new())
    }

    fn predicate_columns(&self, _table_id: i64) -> Result<Vec<i64>, Error> {
        Ok(Vec::new())
    }

    fn gc_index_usage(&self) -> Result<(), Error> {
        Ok(())
    }
}

struct NoopSchema;

impl SchemaState for NoopSchema {
    fn table_exists(&self, _id: i64) -> bool {
        false
    }

    fn table_locked(&self, _id: i64) -> bool {
        false
    }
}

/// 对齐 Go export_test.go 的 getter/setter 契约：默认为 1h，测试可覆盖为短时长。
#[test]
fn canonical_dump_stats_duration_accepts_test_override() {
    let mut usage = StatsUsageImpl::new(Arc::new(NoopStore), Arc::new(NoopSchema));
    assert_eq!(DUMP_STATS_MAX_DURATION, Duration::from_secs(60 * 60));
    assert_eq!(usage.max_delta_age, DUMP_STATS_MAX_DURATION);

    usage.max_delta_age = Duration::from_millis(25);
    assert_eq!(usage.max_delta_age, Duration::from_millis(25));
}
