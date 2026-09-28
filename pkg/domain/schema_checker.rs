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

// Schema（元数据）版本合法性检查器。
//
// 事务开始时记录 schema 版本与相关表；提交前校验该版本在事务时间戳下是否仍有效。
// Validator 可能返回 Success / Fail / Unknown：Unknown 时按退避重试，
// 超过次数则视为 schema 过期（InfoSchemaExpired）。

// limitations under the License.

// SchemaChecker is used for checking schema-validity.
// 对应 Go 的 SchemaChecker：嵌入 Validator，并记录事务开始时的 schema 版本和相关表。
// pub struct SchemaChecker {
//     pub Validator: Box<dyn validatorapi::Validator>,
//     pub schemaVer: i64,
//     pub relatedTableIDs: Vec<i64>,
//     pub needCheckSchemaByDelta: bool,
// }
//
// intSchemaVer 对应 Go 的 int64 别名，用来适配 tikv.SchemaVer 接口。
// pub type intSchemaVer = i64;
//
// SchemaMetaVersion 对应 Go 的同名方法：把 intSchemaVer 暴露为 schema meta version。
// pub fn SchemaMetaVersion(i: intSchemaVer) -> i64 {
//     i as i64
// }
//
// SchemaOutOfDateRetryInterval is the backoff time before retrying.
// Go 中这是 atomic.Duration，Load 后用于 ResultUnknown 的 sleep 退避。
// pub static SchemaOutOfDateRetryInterval: atomicutil::Duration =
//     atomicutil::NewDuration(time::Millisecond * 500);
//
// SchemaOutOfDateRetryTimes is the max retry count when the schema is out of date.
// Go 中这是 atomic.Int32，允许测试或配置在运行期调整重试次数。
// pub static SchemaOutOfDateRetryTimes: atomicutil::Int32 = atomicutil::NewInt32(10);
//
// NewSchemaChecker creates a new schema checker.
// 对应 Go 构造函数：只保存调用方传入的 Validator、schema 版本和相关表 ID。
// pub fn NewSchemaChecker(
//     validator: Box<dyn validatorapi::Validator>,
//     schemaVer: i64,
//     relatedTableIDs: Vec<i64>,
//     needCheckSchemaByDelta: bool,
// ) -> Box<SchemaChecker> {
//     Box::new(SchemaChecker {
//         Validator: validator,
//         schemaVer,
//         relatedTableIDs,
//         needCheckSchemaByDelta,
//     })
// }
//
// impl SchemaChecker {
// Check checks the validity of the schema version.
// Go 这里把记录在结构体内的 schemaVer 包成 intSchemaVer，再委托给 CheckBySchemaVer。
//     pub fn Check(
//         &mut self,
//         txnTS: u64,
//     ) -> Result<Option<Box<transaction::RelatedSchemaChange>>, errors::Error> {
//         self.CheckBySchemaVer(txnTS, self.schemaVer as intSchemaVer)
//     }
//
// CheckBySchemaVer checks if the schema version valid or not at txnTS.
// 保留 Go 的 ResultSucc/ResultFail/ResultUnknown 三分支语义，以及最终过期错误。
//     pub fn CheckBySchemaVer(
//         &mut self,
//         txnTS: u64,
//         startSchemaVer: impl tikv::SchemaVer,
//     ) -> Result<Option<Box<transaction::RelatedSchemaChange>>, errors::Error> {
//         let schemaOutOfDateRetryInterval = SchemaOutOfDateRetryInterval.Load();
//         let schemaOutOfDateRetryTimes = SchemaOutOfDateRetryTimes.Load() as i32;
//
//         for _ in 0..schemaOutOfDateRetryTimes {
//             let (relatedChange, checkResult) = self.Validator.Check(
//                 txnTS,
//                 startSchemaVer.SchemaMetaVersion(),
//                 self.relatedTableIDs.clone(),
//                 self.needCheckSchemaByDelta,
//             );
//
//             match checkResult {
//                 validatorapi::ResultSucc => {
// schema 已确认有效，Go 返回 nil, nil。
//                     return Ok(None);
//                 }
//                 validatorapi::ResultFail => {
// schema 明确变更：记录 changed 指标，并把相关变更连同 ErrInfoSchemaChanged 返回。
//                     metrics::SchemaLeaseErrorCounter
//                         .WithLabelValues("changed")
//                         .Inc();
//                     return Err(ErrInfoSchemaChanged.with_related_change(relatedChange));
//                 }
//                 validatorapi::ResultUnknown => {
//                     time::Sleep(schemaOutOfDateRetryInterval);
//                 }
//                 _ => {}
//             }
//         }
//
// 超过重试次数仍未知，沿用 Go 的 outdated 指标和 ErrInfoSchemaExpired。
//         metrics::SchemaLeaseErrorCounter
//             .WithLabelValues("outdated")
//             .Inc();
//         Err(ErrInfoSchemaExpired)
//     }
// }
// */
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
