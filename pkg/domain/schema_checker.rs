// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// Schema（元数据）版本合法性检查器。
//
// 事务开始时记录 schema 版本与相关表；提交前校验该版本在事务时间戳下是否仍有效。
// Validator 可能返回 Success / Fail / Unknown：Unknown 时按退避重试，
// 超过次数则视为 schema 过期（InfoSchemaExpired）。

use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// 与校验失败相关的 schema 变更信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelatedSchemaChange {
    /// 发生变更的物理表 ID 列表。
    pub physical_table_ids: Vec<i64>,
    /// 对应的 DDL action 类型描述。
    pub action_types: Vec<String>,
}

/// Validator 的三种检查结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchemaCheckResult {
    /// schema 仍然有效。
    Success,
    /// schema 已变更，可选附带相关变更详情。
    Fail(Option<RelatedSchemaChange>),
    /// 暂时无法判定（例如 lease 未同步），可重试。
    Unknown,
}

/// schema 校验器抽象，对齐 Go Validator。
pub trait SchemaValidator: Send + Sync {
    /// 在事务时间戳 `txn_ts` 下检查 `schema_version` 是否仍有效。
    ///
    /// `related_table_ids` 限制检查范围；`check_by_delta` 表示是否按增量变更检查。
    fn check(
        &self,
        txn_ts: u64,
        schema_version: i64,
        related_table_ids: &[i64],
        check_by_delta: bool,
    ) -> SchemaCheckResult;
}

/// SchemaChecker 对外错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchemaCheckError {
    /// 明确检测到 InfoSchema 已变更。
    InfoSchemaChanged(Option<RelatedSchemaChange>),
    /// 多次 Unknown 后仍无法确认，视为过期。
    InfoSchemaExpired,
}

#[allow(static_mut_refs)]
fn increment_schema_lease_error(label: &str) {
    // The metrics crate preserves Go's package-level, lazily initialized handle.
    // A checker can run before metrics initialization, in which case Go's Rust
    // compatibility layer intentionally has no collector to update.
    unsafe {
        if let Some(counter) = &astersql_metrics::SchemaLeaseErrorCounter {
            counter.with_label_values(&[label]).inc();
        }
    }
}

/// 嵌入 Validator，并保存事务起始 schema 版本与相关表。
pub struct SchemaChecker {
    /// 底层校验实现。
    validator: Arc<dyn SchemaValidator>,
    /// 事务开始时的 schema 版本号。
    schema_version: i64,
    /// 本事务涉及的表 ID。
    related_table_ids: Vec<i64>,
    /// 是否按 delta（增量）方式检查。
    check_by_delta: bool,
    /// Unknown 时的退避间隔（默认 500ms）。
    retry_interval: Duration,
    /// 最大重试次数（默认 10）。
    retry_times: usize,
}

impl SchemaChecker {
    /// 构造检查器，使用默认重试参数。
    pub fn new(
        validator: Arc<dyn SchemaValidator>,
        schema_version: i64,
        related_table_ids: Vec<i64>,
        check_by_delta: bool,
    ) -> Self {
        Self {
            validator,
            schema_version,
            related_table_ids,
            check_by_delta,
            retry_interval: Duration::from_millis(500),
            retry_times: 10,
        }
    }

    /// 覆盖重试间隔与次数（测试或调优用）。
    pub fn with_retry(mut self, interval: Duration, times: usize) -> Self {
        self.retry_interval = interval;
        self.retry_times = times;
        self
    }

    /// 使用构造时记录的 schema 版本执行检查。
    pub fn check(&self, txn_ts: u64) -> Result<(), SchemaCheckError> {
        self.check_by_schema_version(txn_ts, self.schema_version)
    }

    /// 按指定 schema 版本检查；Unknown 时睡眠重试，最终失败返回 Expired。
    pub fn check_by_schema_version(
        &self,
        txn_ts: u64,
        schema_version: i64,
    ) -> Result<(), SchemaCheckError> {
        for _ in 0..self.retry_times {
            match self.validator.check(
                txn_ts,
                schema_version,
                &self.related_table_ids,
                self.check_by_delta,
            ) {
                SchemaCheckResult::Success => return Ok(()),
                SchemaCheckResult::Fail(change) => {
                    increment_schema_lease_error("changed");
                    return Err(SchemaCheckError::InfoSchemaChanged(change));
                }
                // Go 在每次 ResultUnknown（包括最后一次）后都先退避。
                SchemaCheckResult::Unknown => thread::sleep(self.retry_interval),
            }
        }
        increment_schema_lease_error("outdated");
        Err(SchemaCheckError::InfoSchemaExpired)
    }
}
