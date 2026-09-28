// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// ANALYZE 测试辅助：谓词列收集触发与运行时适配接口。
//
// 通过 [`AnalyzeRuntime`] 对活会话执行等值谓词查询，再 dump 列统计用量到 KV，
// 以驱动优化器谓词列（predicate columns）收集路径。

use std::fmt;

/// ANALYZE 辅助错误，承载可读消息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalyzeError(pub String);

impl fmt::Display for AnalyzeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for AnalyzeError {}

/// Adapter implemented by a live TestKit/session pair.
/// 由活的 TestKit/会话对实现的运行时适配器。
pub trait AnalyzeRuntime {
    /// 执行一条 SQL。
    fn execute(&mut self, sql: &str) -> Result<(), AnalyzeError>;
    /// 将列统计用量落盘到 KV。
    fn dump_column_stats_usage_to_kv(&mut self) -> Result<(), AnalyzeError>;
}

/// 对指定表各列执行等值谓词 SELECT，再 dump 列统计用量。
pub fn TriggerPredicateColumnsCollection(
    runtime: &mut dyn AnalyzeRuntime,
    table_name: &str,
    columns: &[String],
) -> Result<(), AnalyzeError> {
    // 每列一条 `WHERE col = '1'`，触发谓词列使用记录。
    for column in columns {
        runtime.execute(&format!(
            "SELECT * FROM {} WHERE {} = '1'",
            table_name, column
        ))?;
    }
    runtime.dump_column_stats_usage_to_kv()
}
