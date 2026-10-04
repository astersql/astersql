// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// DDL 回填（backfill）任务完成后的云存储（S3 等）清理模块。
//
// 背景：在分布式添加索引（Add Index）场景中，回填流程会把扫描出的
// 索引键值对（KV）先写入外部云存储（如 S3）作为中间产物，待全局排序
// 与导入完成后，这些中间文件就不再需要。本模块负责在任务结束时：
// 1. 按任务 ID / 作业 ID 前缀删除云存储中的中间文件；
// 2. 在下一代内核（next generation kernel）模式下上报计量数据
//    （处理的行数与索引 KV 字节数），用于计费或资源统计；
// 3. 对任务元数据中的云存储 URI 做脱敏（redact），避免凭据泄露。

use crate::backfilling_dist_executor::{BACKFILL_TASK_META_VERSION_1, BackfillTaskMeta};
use crate::backfilling_read_index::SubtaskSummary;

/// 清理任务所处的状态机状态。
///
/// 状态影响清理行为：只有 `Succeed`（成功结束）的任务才会触发计量上报。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskState {
    /// 等待调度，尚未开始执行。
    Pending,
    /// 正在执行中。
    Running,
    /// 已成功完成。
    Succeed,
    /// 执行失败。
    Failed,
}

/// 一次待清理的回填任务的描述信息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CleanupTask {
    /// 分布式框架分配的任务 ID，同时也是云存储中中间文件的路径前缀。
    pub id: i64,
    /// 任务的最终状态，决定是否需要上报计量数据。
    pub state: TaskState,
    /// 回填任务元数据，包含云存储 URI、DDL 作业 ID、元数据版本等。
    pub meta: BackfillTaskMeta,
}

/// 清理阶段上报的计量数据（metering data）。
///
/// 计量用于统计本次回填实际处理的数据量，常见用途是云服务计费。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MeteringData {
    /// 回填处理的总行数。
    pub row_count: i64,
    /// 写入的索引 KV 数据总字节数。
    pub index_kv_size: i64,
}

/// 云存储清理操作的抽象接口。
///
/// 通过 trait 解耦具体的对象存储实现（S3、GCS 等），便于测试时注入桩实现。
pub trait CleanupStorage {
    /// 删除云存储中以 `prefix` 为路径前缀的全部对象。
    fn cleanup_prefix(&mut self, prefix: &str) -> Result<(), String>;
}

/// 清理流程中可能出现的错误类型。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CleanupError {
    /// 任务元数据中的云存储 URI 格式非法（缺少 scheme 或 host）。
    InvalidCloudStorageUri,
    /// 调用存储层删除对象时出错，内含底层错误信息。
    Storage(String),
    /// 上报计量数据时出错，内含底层错误信息。
    Meter(String),
}

/// 回填任务的 S3 清理器（无状态，逻辑集中在 [`Self::clean`] 中）。
#[derive(Clone, Debug, Default)]
pub struct BackfillCleaner;

impl BackfillCleaner {
    /// 执行清理主流程：删除云存储中间文件、按需上报计量数据、脱敏 URI。
    ///
    /// 参数说明：
    /// - `task`：待清理的任务，其元数据中的云存储 URI 会被就地脱敏；
    /// - `storage`：云存储清理接口的具体实现；
    /// - `next_generation_kernel`：是否运行在下一代内核模式（该模式下需上报计量）；
    /// - `successful_read_summaries`：各成功读索引子任务的汇总统计，用于聚合计量数据；
    /// - `send_meter`：计量数据上报回调。
    pub fn clean(
        &self,
        task: &mut CleanupTask,
        storage: &mut dyn CleanupStorage,
        next_generation_kernel: bool,
        successful_read_summaries: &[SubtaskSummary],
        mut send_meter: impl FnMut(MeteringData) -> Result<(), String>,
    ) -> Result<(), CleanupError> {
        // 未配置云存储 URI 说明本次回填走本地路径，无中间文件需要清理。
        if task.meta.cloud_storage_uri.is_empty() {
            return Ok(());
        }
        if !valid_cloud_storage_uri(&task.meta.cloud_storage_uri) {
            return Err(CleanupError::InvalidCloudStorageUri);
        }
        // 中间文件以任务 ID 作为路径前缀存放，按前缀整体删除。
        storage
            .cleanup_prefix(&task.id.to_string())
            .map_err(CleanupError::Storage)?;
        // 兼容旧版本元数据：版本 1 之前的中间文件以 DDL 作业 ID 为前缀，
        // 需要额外按作业 ID 前缀再清理一次。
        if task.meta.version < BACKFILL_TASK_META_VERSION_1 {
            storage
                .cleanup_prefix(&task.meta.job_id.to_string())
                .map_err(CleanupError::Storage)?;
        }
        // 计量上报的三个条件：下一代内核模式、任务成功结束、
        // 且不是"合并临时索引"任务（合并阶段不重复计量已统计过的数据）。
        if next_generation_kernel
            && task.state == TaskState::Succeed
            && !task.meta.merge_temporary_index
        {
            send_meter(send_meter_on_clean(successful_read_summaries))
                .map_err(CleanupError::Meter)?;
        }
        // 清理完成后脱敏 URI，防止其中携带的访问凭据落盘或出现在日志中。
        task.meta.cloud_storage_uri = redact_cloud_storage_uri(&task.meta.cloud_storage_uri);
        Ok(())
    }
}

/// 聚合所有成功子任务的统计信息，得到清理阶段要上报的计量数据。
///
/// 行数与处理字节数分别对各子任务汇总求和。
pub fn send_meter_on_clean(summaries: &[SubtaskSummary]) -> MeteringData {
    MeteringData {
        row_count: summaries.iter().map(|summary| summary.row_count).sum(),
        index_kv_size: summaries
            .iter()
            .map(|summary| summary.processed_bytes)
            .sum(),
    }
}

/// 对云存储 URI 做脱敏处理，去掉可能包含敏感信息的部分。
///
/// 具体规则与 Go 的 `ast.RedactURL` 一致：只对云存储 scheme 的已知
/// 敏感 query key 替换为 `xxxxxx`，保留 userinfo、路径和其他 query 参数。
pub fn redact_cloud_storage_uri(uri: &str) -> String {
    astersql_parser_ast::misc::redact_url(uri)
}

/// 校验云存储 URI 是否合法：必须形如 `scheme://authority...`，
/// 且 scheme 与 authority 均非空。
fn valid_cloud_storage_uri(uri: &str) -> bool {
    uri.split_once("://")
        .is_some_and(|(scheme, authority)| !scheme.is_empty() && !authority.is_empty())
}
