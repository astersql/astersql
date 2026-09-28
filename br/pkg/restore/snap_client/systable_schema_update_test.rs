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

//! Go-equivalent tests for `systable_schema_update_test.go`.
//! Domain/SQL boundaries: TableInfo fixtures (no kv/domain).
//! 覆盖 stats_meta 版本推断与升降级 SQL 选择，不拉起真实 Domain。
//! 通过捕获 execution 回调确认：同版本无 SQL，跨版本仅一条 ADD/DROP。
//! 断言关键字与 Go 测试一致，保证临时表 schema 路径不被误改。
//! fixture 只构造列名，不依赖列类型/默认值，聚焦版本判定契约。
//! V1→V2 与 V2→V1 两条路径分别锁定 DROP 与 ADD，防止方向写反。
//! 同版本用例用 panic 回调，确保零 SQL 副作用被严格执行。
//! 导出 API 走 export_test，避免测试直接依赖未 re-export 的内部符号。
//! SQL 断言同时检查动词与列名，防止匹配到无关 ALTER。
//! 场景顺序与 Go 测试相近，便于双端对照失败用例。

use crate::export_test::{GetSchemaVersionFromStatsMeta, UpdateStatsMetaSchema};
use crate::stubs::{Error, model};
use crate::systable_schema_update::SchemaVersionType;

const ADD_HISTOGRAM_VERSION_SQL: &str = "ALTER TABLE __TiDB_BR_Temporary_mysql.stats_meta ADD COLUMN IF NOT EXISTS last_stats_histograms_version bigint unsigned DEFAULT NULL";
const DROP_HISTOGRAM_VERSION_SQL: &str = "ALTER TABLE __TiDB_BR_Temporary_mysql.stats_meta DROP COLUMN IF EXISTS last_stats_histograms_version";

/// 构造 V1 表结构：不含 last_stats_histograms_version 列。
fn stats_meta_v1() -> model::TableInfo {
    model::TableInfo {
        Name: model::CIStr::new("stats_meta"),
        Columns: vec![
            model::ColumnInfo {
                Name: model::CIStr::new("version"),
                ..Default::default()
            },
            model::ColumnInfo {
                Name: model::CIStr::new("table_id"),
                ..Default::default()
            },
            model::ColumnInfo {
                Name: model::CIStr::new("modify_count"),
                ..Default::default()
            },
            model::ColumnInfo {
                Name: model::CIStr::new("count"),
                ..Default::default()
            },
            model::ColumnInfo {
                Name: model::CIStr::new("snapshot"),
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}

/// 在 V1 上追加直方图版本列，得到 V2 fixture。
fn stats_meta_v2() -> model::TableInfo {
    let mut t = stats_meta_v1();
    t.Columns.push(model::ColumnInfo {
        Name: model::CIStr::new("last_stats_histograms_version"),
        ..Default::default()
    });
    t
}

/// TestGetSchemaVersionFromStatsMeta — Go `TestGetSchemaVersionFromStatsMeta`.
/// 场景：版本识别 + 同版本幂等 + 跨版本 ADD/DROP 各一条。
#[test]
fn test_get_schema_version_from_stats_meta() {
    let downstream_v2 = stats_meta_v2();
    let downstream_v1 = stats_meta_v1();
    let upstream_v2 = stats_meta_v2();
    let upstream_v1 = stats_meta_v1();

    // 列存在性决定 Version2 / Version1。
    assert_eq!(
        GetSchemaVersionFromStatsMeta(&downstream_v2),
        SchemaVersionType::Version2
    );
    assert_eq!(
        GetSchemaVersionFromStatsMeta(&downstream_v1),
        SchemaVersionType::Version1
    );

    // case 1-1: both Version2 → no SQL
    // 双端均为 V2：回调若被调用则说明误触发 ALTER。
    UpdateStatsMetaSchema(&downstream_v2, &upstream_v2, |_| {
        panic!("should not execute any sql");
    })
    .unwrap();

    // case: downstream V1, upstream V2 → downgrade temporary (DROP COLUMN)
    // 临时表偏旧、上游偏新：应 DROP 列以对齐上游。
    let mut sqls = Vec::new();
    UpdateStatsMetaSchema(&downstream_v1, &upstream_v2, |s| {
        sqls.push(s.to_string());
        Ok(())
    })
    .unwrap();
    assert_eq!(sqls, [DROP_HISTOGRAM_VERSION_SQL]);

    // case: both Version1 → no SQL
    UpdateStatsMetaSchema(&downstream_v1, &upstream_v1, |_| {
        panic!("should not execute any sql");
    })
    .unwrap();

    // case: downstream V2, upstream V1 → upgrade temporary (ADD COLUMN)
    // 临时表偏新、上游偏旧：应 ADD 列承接下游 schema。
    sqls.clear();
    UpdateStatsMetaSchema(&downstream_v2, &upstream_v1, |s| {
        sqls.push(s.to_string());
        Ok(())
    })
    .unwrap();
    assert_eq!(sqls, [ADD_HISTOGRAM_VERSION_SQL]);

    // Go returns errors.Trace(execution(...)); Rust must likewise stop and
    // preserve the callback error instead of reporting a successful update.
    let mut attempts = 0;
    let err = UpdateStatsMetaSchema(&downstream_v2, &upstream_v1, |sql| {
        attempts += 1;
        assert_eq!(sql, ADD_HISTOGRAM_VERSION_SQL);
        Err(Error::Errorf("injected execution failure"))
    })
    .unwrap_err();
    assert_eq!(attempts, 1);
    assert!(err.to_string().contains("injected execution failure"));
}
