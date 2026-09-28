// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// Region（分布式 KV 存储中的数据分片单位）预切分与 scatter 辅助。
//
// 建表/加索引时可按 shard row id、显式 split policy 或表前缀生成切分键，
// 调用存储层 SplitRegions，并可按 Table/Global 作用域等待 scatter
//（将新建 Region 打散到不同 store，避免热点集中）完成。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Scatter 作用域：全局统一组，或按表 ID 分组。
pub enum ScatterScope {
    Global,
    Table,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 显式 Region 切分策略：上下界、目标 Region 数，或直接给出 value lists。
pub struct RegionSplitPolicy {
    pub lower: Vec<String>,
    pub upper: Vec<String>,
    pub num: u64,
    pub value_lists: Vec<Vec<String>>,
    pub index_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 参与预切分的表信息：物理 ID、分片位数、预切 Region 数与索引 ID。
pub struct SplitTableInfo {
    pub table_id: i64,
    pub partition_ids: Vec<i64>,
    pub shard_row_id_bits: u8,
    pub pre_split_regions: u8,
    pub index_ids: Vec<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 切分或 scatter 过程中的错误。
pub enum SplitError {
    InvalidBounds,
    InvalidRegionCount,
    TooManyPreSplitRegions,
    InvalidExpression,
    ScatterFailed(u64),
}

impl std::fmt::Display for SplitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for SplitError {}

/// 校验并归一化 SPLIT REGION 选项，生成 `RegionSplitPolicy`。
///
/// `value_lists` 为空时要求 `num >= 1` 且上下界等长；
/// 否则禁止同时指定 num/上下界，且每组 value 非空。
pub fn normalize_split_policy(
    lower: Vec<String>,
    upper: Vec<String>,
    num: u64,
    value_lists: Vec<Vec<String>>,
    index_name: Option<String>,
) -> Result<RegionSplitPolicy, SplitError> {
    if value_lists.is_empty() {
        if num < 1 {
            return Err(SplitError::InvalidRegionCount);
        }
        if lower.is_empty() || lower.len() != upper.len() {
            return Err(SplitError::InvalidBounds);
        }
    } else if num != 0 || !lower.is_empty() || !upper.is_empty() {
        return Err(SplitError::InvalidBounds);
    }
    if value_lists
        .iter()
        .any(|values| values.is_empty() || values.iter().any(|value| value.trim().is_empty()))
    {
        return Err(SplitError::InvalidExpression);
    }
    Ok(RegionSplitPolicy {
        lower,
        upper,
        num,
        value_lists,
        index_name: index_name.map(|name| name.to_ascii_lowercase()),
    })
}

/// 编码表记录键：`t{physical_id}_r{memcomparable(handle)}`。
pub fn encode_record_key(physical_id: i64, handle: i64) -> Vec<u8> {
    let mut key = Vec::with_capacity(18);
    key.extend_from_slice(b"t");
    key.extend_from_slice(&physical_id.to_be_bytes());
    key.extend_from_slice(b"_r");
    // 将有符号 handle 转为 memcomparable 无序编码（翻转符号位）。
    key.extend_from_slice(&(handle as u64 ^ (1_u64 << 63)).to_be_bytes());
    key
}

/// 编码索引键：`t{physical_id}_i{index_id}` 后跟各列值与 0 分隔。
pub fn encode_index_key(physical_id: i64, index_id: i64, values: &[String]) -> Vec<u8> {
    let mut key = Vec::new();
    key.extend_from_slice(b"t");
    key.extend_from_slice(&physical_id.to_be_bytes());
    key.extend_from_slice(b"_i");
    key.extend_from_slice(&index_id.to_be_bytes());
    for value in values {
        key.extend_from_slice(value.as_bytes());
        key.push(0);
    }
    key
}

/// 按 shard_row_id_bits / pre_split_regions 为各物理表生成记录区预切分键。
pub fn pre_split_record_keys(info: &SplitTableInfo) -> Result<Vec<Vec<u8>>, SplitError> {
    if info.pre_split_regions > info.shard_row_id_bits
        || info.pre_split_regions > 15
        || info.shard_row_id_bits > 63
    {
        return Err(SplitError::TooManyPreSplitRegions);
    }
    // 无分区时用 table_id；有分区则对每个 partition_id 分别切分。
    let physical_ids: Vec<i64> = if info.partition_ids.is_empty() {
        vec![info.table_id]
    } else {
        info.partition_ids.clone()
    };
    let shard_count = 1_u64 << info.shard_row_id_bits;
    let shard_step = 1_u64 << (info.shard_row_id_bits - info.pre_split_regions);
    let incremental_bits = 63 - info.shard_row_id_bits;
    let mut keys = Vec::new();
    for physical_id in physical_ids {
        let mut table_prefix = b"t".to_vec();
        table_prefix.extend_from_slice(&physical_id.to_be_bytes());
        keys.push(table_prefix);
        let mut shard = shard_step;
        while shard < shard_count {
            let handle = shard << incremental_bits;
            keys.push(encode_record_key(physical_id, handle as i64));
            shard += shard_step;
        }
    }
    Ok(keys)
}

/// 按显式 policy 生成切分键：索引 value lists、记录 value lists，或上下界均匀插值。
pub fn policy_split_keys(
    info: &SplitTableInfo,
    policy: &RegionSplitPolicy,
) -> Result<Vec<Vec<u8>>, SplitError> {
    let physical_ids: Vec<i64> = if info.partition_ids.is_empty() {
        vec![info.table_id]
    } else {
        info.partition_ids.clone()
    };
    let mut keys = Vec::new();
    // 约定索引名形如 `idx{N}` 时按索引键切分。
    let index_id = policy.index_name.as_ref().and_then(|name| {
        name.strip_prefix("idx")
            .and_then(|id| id.parse::<i64>().ok())
    });
    for physical_id in physical_ids {
        if let Some(index_id) = index_id {
            for values in &policy.value_lists {
                keys.push(encode_index_key(physical_id, index_id, values));
            }
        } else if !policy.value_lists.is_empty() {
            for values in &policy.value_lists {
                let handle = values
                    .first()
                    .and_then(|value| value.parse().ok())
                    .ok_or(SplitError::InvalidExpression)?;
                keys.push(encode_record_key(physical_id, handle));
            }
        } else {
            // 在 [lower, upper) 上按 num 等分插入切分点。
            let low = policy
                .lower
                .first()
                .and_then(|value| value.parse::<i64>().ok())
                .ok_or(SplitError::InvalidExpression)?;
            let high = policy
                .upper
                .first()
                .and_then(|value| value.parse::<i64>().ok())
                .ok_or(SplitError::InvalidExpression)?;
            let span = high as i128 - low as i128;
            for offset in 1..policy.num {
                let handle = low as i128 + span * offset as i128 / policy.num as i128;
                keys.push(encode_record_key(physical_id, handle as i64));
            }
        }
    }
    keys.sort();
    keys.dedup();
    Ok(keys)
}

/// 将布尔开关映射为 Scatter 作用域。
pub fn scatter_scope(global_scatter: bool) -> ScatterScope {
    if global_scatter {
        ScatterScope::Global
    } else {
        ScatterScope::Table
    }
}

/// 等待 scatter 结果：全部成功返回完成数，遇到非 PD 错误立即停止。
///
/// 此简化接口无法表达 Go 中可继续等待的 `PDError`，因此其 `Err(())`
/// 代表 Go 分支中的非 PD 错误。
pub fn wait_scatter_finished(
    results: impl IntoIterator<Item = Result<(), ()>>,
) -> Result<u64, SplitError> {
    let mut finished = 0;
    for result in results {
        match result {
            Ok(()) => finished += 1,
            Err(()) => return Err(SplitError::ScatterFailed(1)),
        }
    }
    Ok(finished)
}
