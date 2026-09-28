// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// DDL Reorg 任务元数据与表大小估算辅助函数。
//
// 将会话侧 `ReorgVariables` 规范化为 job 可持久化的 `InitializedReorgMeta`，
// 并基于 Region（数据分片）近似大小估算表体积。

/// 会话/配置侧传入的 reorg 运行参数。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReorgVariables {
    /// 并发 worker 数。
    pub worker_count: usize,
    /// 每批处理行数。
    pub batch_size: usize,
    /// 最大写吞吐限制；0 表示不限制。
    pub max_write_speed: usize,
    /// 是否使用云存储作为中间介质。
    pub use_cloud_storage: bool,
    /// 云存储 URI；启用云存储时必须非空。
    pub cloud_storage_uri: String,
    /// 是否启用分布式回填。
    pub distributed: bool,
}

/// 默认：4 worker、批大小 256、本地存储、非分布式。
impl Default for ReorgVariables {
    fn default() -> Self {
        Self {
            worker_count: 4,
            batch_size: 256,
            max_write_speed: 0,
            use_cloud_storage: false,
            cloud_storage_uri: String::new(),
            distributed: false,
        }
    }
}

/// 校验并写入 DDL job 的 reorg 元数据快照。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InitializedReorgMeta {
    /// 并发度（来自 worker_count）。
    pub concurrency: usize,
    /// 批大小。
    pub batch_size: usize,
    /// 写速度上限。
    pub max_write_speed: usize,
    /// 云存储 URI；未启用时为空串。
    pub cloud_storage_uri: String,
    /// 是否分布式。
    pub distributed: bool,
    /// 元数据版本号，供跨版本兼容（如 end_key 边界调整）使用。
    pub version: u32,
}

/// 初始化 reorg 元数据时的参数错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReorgMetaError {
    /// worker_count 为 0。
    InvalidConcurrency,
    /// batch_size 为 0。
    InvalidBatchSize,
    /// 开启云存储但 URI 为空。
    CloudStorageUriMissing,
}

/// 校验变量并生成可持久化的 reorg meta；未启用云存储时 URI 置空。
pub fn init_job_reorg_meta_from_variables(
    variables: &ReorgVariables,
) -> Result<InitializedReorgMeta, ReorgMetaError> {
    // 并发与批大小必须为正；云存储路径下 URI 必填。
    if variables.worker_count == 0 {
        return Err(ReorgMetaError::InvalidConcurrency);
    }
    if variables.batch_size == 0 {
        return Err(ReorgMetaError::InvalidBatchSize);
    }
    if variables.use_cloud_storage && variables.cloud_storage_uri.is_empty() {
        return Err(ReorgMetaError::CloudStorageUriMissing);
    }
    Ok(InitializedReorgMeta {
        concurrency: variables.worker_count,
        batch_size: variables.batch_size,
        max_write_speed: variables.max_write_speed,
        cloud_storage_uri: variables
            .use_cloud_storage
            .then(|| variables.cloud_storage_uri.clone())
            .unwrap_or_default(),
        distributed: variables.distributed,
        version: 1,
    })
}

/// 单个 Region 的近似大小（MiB），取存储层与 KV 层估计的较大值参与汇总。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RegionSize {
    /// Region 近似总大小（MiB）。
    pub approximate_size_mib: i64,
    /// Region 近似 KV 数据大小（MiB）。
    pub approximate_kv_size_mib: i64,
}

/// 将多页 Region 大小估计汇总为表字节数（MiB × 1024²）。
pub fn estimate_table_size_by_regions(pages: &[Vec<RegionSize>]) -> i64 {
    // PD/Store 返回的是 MiB，这里换算为字节以便与其它估算对齐。
    const MEBIBYTE: i64 = 1024 * 1024;
    pages.iter().flatten().fold(0, |total, region| {
        region
            .approximate_size_mib
            .max(region.approximate_kv_size_mib)
            .wrapping_mul(MEBIBYTE)
            .wrapping_add(total)
    })
}

/// 按分区大小列表求和得到表总大小，保留 Go `int64` 的回绕语义。
pub fn get_table_size_by_id(partition_sizes: &[i64]) -> i64 {
    partition_sizes.iter().copied().fold(0, i64::wrapping_add)
}
