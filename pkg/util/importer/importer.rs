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

// 导入主流程入口：解析 DDL、建表建索引、调度并发 job。
//
// 对应 Go importer 的 `Process`：先解析 table/index SQL 填充元数据，
// 再执行 DDL，最后调用 `process_jobs` 压测式批量插入。

use crate::config::{Config, ImporterError};
use crate::db::{DatabaseConnector, close_databases, create_databases, execute_sql};
use crate::job::{ProcessReport, process_jobs};
use crate::parser::{Table, parse_index_sql, parse_table_sql};
use std::sync::Arc;

/// 执行完整导入：解析配置中的 DDL → 打开连接 → 建表/建索引 → 并发插入 → 关闭连接。
///
/// 关闭连接产生的错误仅被忽略（对齐 Go：记录日志但不覆盖导入结果）。
pub fn process(
    config: &Config,
    connector: &dyn DatabaseConnector,
) -> Result<ProcessReport, ImporterError> {
    let mut table = Table::new();
    parse_table_sql(&mut table, &config.table_sql)?;
    parse_index_sql(&mut table, &config.index_sql)?;
    if config.worker_count == 0 {
        return Err(ImporterError::InvalidConfig(
            "worker-count must be positive".to_owned(),
        ));
    }
    let databases = create_databases(connector, &config.db_config, config.worker_count)?;
    // 用闭包包住导入主体，便于无论成败都执行 close_databases。
    let result = (|| {
        execute_sql(databases[0].as_ref(), &config.table_sql)?;
        execute_sql(databases[0].as_ref(), &config.index_sql)?;
        process_jobs(
            Arc::new(table),
            &databases,
            config.job_count,
            config.worker_count,
            config.batch,
        )
    })();
    // Go logs close errors and never replaces the import result with them.
    let _close_errors = close_databases(&databases);
    result
}
