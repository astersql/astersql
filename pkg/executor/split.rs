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

// SPLIT REGION 执行器：按表/索引键切分 TiKV Region，并可选等待 scatter 完成。
//
// Region 是 TiKV 的键空间分片；PD（Placement Driver）负责调度。SPLIT REGION 在指定
// 边界键处切开 Region，scatter 则把新 Region 打散到不同 store。本模块通过
// [`SplitRuntime`] 抽象编码、PD/TiKV 调用与元数据加载，上层提供具体实现。

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

use std::collections::HashSet;
use std::time::{Duration, Instant};

/// 上下文已取消时，等待 scatter 的退避超时（毫秒）。
pub const checkScatterRegionFinishBackOff: i32 = 50;

/// 一次 SPLIT 的结果计数：切出的 Region 数与完成 scatter 的数量。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct splitRegionResult {
    /// 成功发起 split 得到的 Region 数。
    pub splitRegions: i32,
    /// 在超时内完成 scatter 的 Region 数。
    pub finishScatterNum: i32,
}

/// PD/TiKV 返回的 Region 描述（id、leader、store、起止键）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RegionDescriptor {
    pub id: u64,
    pub leader_id: u64,
    pub store_id: u64,
    pub start_key: Vec<u8>,
    pub end_key: Vec<u8>,
}

/// Region 读写与近似规模统计，用于 SHOW 类展示。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RegionStatistics {
    pub written_bytes: u64,
    pub read_bytes: u64,
    pub approximate_size: i64,
    pub approximate_keys: i64,
}

/// 带可读起止键、scatter 状态与统计信息的 Region 元数据。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct regionMeta {
    pub region: RegionDescriptor,
    pub leaderID: u64,
    pub storeID: u64,
    pub start: String,
    pub end: String,
    pub scattering: bool,
    pub writtenBytes: u64,
    pub readBytes: u64,
    pub approximateSize: i64,
    pub approximateKeys: i64,
    pub physicalID: i64,
}

/// Required TiKV/PD/tablecodec boundary for SPLIT REGION.
///
/// The executor owns partition expansion, one-shot execution, scatter waiting,
/// region de-duplication and readable key formatting. Implementations provide
/// real metadata, encoding and storage operations; none has a default success.
///
/// SPLIT REGION 所需的 TiKV/PD/tablecodec 边界。
/// 执行器负责分区展开、一次性执行、scatter 等待、Region 去重与可读键格式化；
/// 实现方提供真实元数据、编码与存储操作。
pub trait SplitRuntime {
    type Context;
    type Chunk;
    type Datum: Clone;
    type HandleColumns: Clone;
    type Error;

    fn reset_chunk(&mut self, chunk: &mut Self::Chunk);
    fn append_int64(&mut self, chunk: &mut Self::Chunk, column: usize, value: i64);
    fn append_float64(&mut self, chunk: &mut Self::Chunk, column: usize, value: f64);

    fn partition_ids(&self) -> Vec<(String, i64)>;
    fn table_id(&self) -> i64;
    fn table_name(&self) -> String;
    fn index_id(&self) -> i64;
    fn index_name(&self) -> String;
    fn public_index_ids(&self) -> Vec<i64>;
    fn unsigned_int_handle(&self) -> bool;

    fn index_start_and_boundary_keys(
        &mut self,
        physical_id: i64,
        keys: Vec<Vec<u8>>,
    ) -> Result<Vec<Vec<u8>>, Self::Error>;
    fn encode_index_value_key(
        &mut self,
        physical_id: i64,
        values: &[Self::Datum],
    ) -> Result<Vec<u8>, Self::Error>;
    fn split_index_bound_keys(
        &mut self,
        physical_id: i64,
        lower: &[Self::Datum],
        upper: &[Self::Datum],
        number: i32,
        keys: Vec<Vec<u8>>,
    ) -> Result<Vec<Vec<u8>>, Self::Error>;
    fn encode_table_value_key(
        &mut self,
        physical_id: i64,
        handle_columns: &Self::HandleColumns,
        values: &[Self::Datum],
    ) -> Result<Vec<u8>, Self::Error>;
    fn split_table_bound_keys(
        &mut self,
        physical_id: i64,
        handle_columns: &Self::HandleColumns,
        lower: &[Self::Datum],
        upper: &[Self::Datum],
        number: i32,
        keys: Vec<Vec<u8>>,
    ) -> Result<Vec<Vec<u8>>, Self::Error>;

    fn split_regions(
        &mut self,
        context: &mut Self::Context,
        keys: Vec<Vec<u8>>,
        table_id: i64,
    ) -> Result<Vec<u64>, Self::Error>;
    fn wait_split_timeout(&self) -> Duration;
    fn wait_split_region_finish(&self) -> bool;
    fn context_done(&self, context: &Self::Context) -> bool;
    fn wait_scatter_region_finish(
        &mut self,
        context: &mut Self::Context,
        region_id: u64,
        timeout_milliseconds: i32,
    ) -> Result<(), Self::Error>;
    fn warn_split_failed(&self, table: &str, index: Option<&str>, error: &Self::Error);
    fn warn_scatter_failed(
        &self,
        region_id: u64,
        table: &str,
        index: Option<&str>,
        error: &Self::Error,
    );

    fn table_handle_key_range(&self, physical_id: i64) -> (Vec<u8>, Vec<u8>);
    fn table_index_key_range(&self, physical_id: i64, index_id: i64) -> (Vec<u8>, Vec<u8>);
    fn load_regions(
        &mut self,
        start: Vec<u8>,
        end: Vec<u8>,
    ) -> Result<Vec<RegionDescriptor>, Self::Error>;
    fn region_is_scattering(&mut self, region_id: u64) -> Result<bool, Self::Error>;
    fn region_statistics(
        &mut self,
        region_id: u64,
    ) -> Result<Option<RegionStatistics>, Self::Error>;
    fn table_prefix(&self, physical_id: i64) -> Vec<u8>;
    fn record_prefix(&self, physical_id: i64) -> Vec<u8>;
    fn index_prefix(&self, physical_id: i64, index_id: i64) -> Vec<u8>;
}

/// 按索引键切分 Region 的执行器（`SPLIT TABLE ... INDEX ...`）。
pub struct SplitIndexRegionExec<R: SplitRuntime> {
    pub runtime: R,
    pub partitionNames: Vec<String>,
    pub lower: Vec<R::Datum>,
    pub upper: Vec<R::Datum>,
    pub num: i32,
    pub valueLists: Vec<Vec<R::Datum>>,
    pub splitIdxKeys: Vec<Vec<u8>>,
    pub done: bool,
    pub splitRegionResult: splitRegionResult,
}

impl<R> SplitIndexRegionExec<R>
where
    R: SplitRuntime,
    R::Error: From<String>,
{
    /// 预计算索引切分键并重置结果状态。
    pub fn Open(&mut self, _context: &mut R::Context) -> Result<(), R::Error> {
        self.splitIdxKeys = self.getSplitIdxKeys()?;
        self.done = false;
        self.splitRegionResult = splitRegionResult::default();
        Ok(())
    }

    /// 一次性执行索引 Region 切分，并把结果写入输出 chunk。
    pub fn Next(&mut self, context: &mut R::Context, chunk: &mut R::Chunk) -> Result<(), R::Error> {
        self.runtime.reset_chunk(chunk);
        if self.done {
            return Ok(());
        }
        // 只执行一轮：后续 Next 返回空 chunk
        self.done = true;
        self.splitIndexRegion(context)?;
        appendSplitRegionResultToChunk(&mut self.runtime, chunk, self.splitRegionResult);
        Ok(())
    }

    /// 调用 runtime 切分索引 Region，并按需等待 scatter。
    pub fn splitIndexRegion(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        let start = Instant::now();
        let table_id = self.runtime.table_id();
        let ids = match self
            .runtime
            .split_regions(context, self.splitIdxKeys.clone(), table_id)
        {
            Ok(ids) => ids,
            Err(error) => {
                // 切分失败只告警，不向上返回错误（与 Go 行为一致）
                self.runtime.warn_split_failed(
                    &self.runtime.table_name(),
                    Some(&self.runtime.index_name()),
                    &error,
                );
                Vec::new()
            }
        };
        self.splitRegionResult.splitRegions = ids.len() as i32;
        if !ids.is_empty() && self.runtime.wait_split_region_finish() {
            let index_name = self.runtime.index_name();
            self.splitRegionResult.finishScatterNum =
                waitScatterRegionFinish(&mut self.runtime, context, start, ids, Some(index_name));
        }
        Ok(())
    }

    /// 按值列表或上下界生成索引切分键。
    pub fn getSplitIdxKeys(&mut self) -> Result<Vec<Vec<u8>>, R::Error> {
        if self.valueLists.is_empty() {
            self.getSplitIdxKeysFromBound()
        } else {
            self.getSplitIdxKeysFromValueList()
        }
    }

    /// 解析目标物理表/分区 ID 列表。
    fn physical_ids(&self) -> Result<Vec<i64>, R::Error> {
        selectedPhysicalIDs(&self.runtime, &self.partitionNames).map_err(Into::into)
    }

    /// 对每个物理表按 valueLists 编码索引键。
    pub fn getSplitIdxKeysFromValueList(&mut self) -> Result<Vec<Vec<u8>>, R::Error> {
        let mut keys = Vec::new();
        for physical_id in self.physical_ids()? {
            keys = self.getSplitIdxPhysicalKeysFromValueList(physical_id, keys)?;
        }
        Ok(keys)
    }

    /// 单个物理表：附加索引起止边界键，再编码各 value 行。
    pub fn getSplitIdxPhysicalKeysFromValueList(
        &mut self,
        physical_id: i64,
        keys: Vec<Vec<u8>>,
    ) -> Result<Vec<Vec<u8>>, R::Error> {
        let mut keys = self
            .runtime
            .index_start_and_boundary_keys(physical_id, keys)?;
        for values in &self.valueLists {
            keys.push(self.runtime.encode_index_value_key(physical_id, values)?);
        }
        Ok(keys)
    }

    /// 按 lower/upper/num 在每个物理表上均匀生成索引切分键。
    pub fn getSplitIdxKeysFromBound(&mut self) -> Result<Vec<Vec<u8>>, R::Error> {
        let mut keys = Vec::new();
        for physical_id in self.physical_ids()? {
            keys = self.getSplitIdxPhysicalKeysFromBound(physical_id, keys)?;
        }
        Ok(keys)
    }

    /// 委托 runtime 按索引上下界切分键。
    pub fn getSplitIdxPhysicalKeysFromBound(
        &mut self,
        physical_id: i64,
        keys: Vec<Vec<u8>>,
    ) -> Result<Vec<Vec<u8>>, R::Error> {
        self.runtime
            .split_index_bound_keys(physical_id, &self.lower, &self.upper, self.num, keys)
    }
}

/// 按表（行 handle）键切分 Region 的执行器（`SPLIT TABLE ...`）。
pub struct SplitTableRegionExec<R: SplitRuntime> {
    pub runtime: R,
    pub partitionNames: Vec<String>,
    pub lower: Vec<R::Datum>,
    pub upper: Vec<R::Datum>,
    pub num: i32,
    pub handleCols: R::HandleColumns,
    pub valueLists: Vec<Vec<R::Datum>>,
    pub splitKeys: Vec<Vec<u8>>,
    pub done: bool,
    pub splitRegionResult: splitRegionResult,
}

impl<R> SplitTableRegionExec<R>
where
    R: SplitRuntime,
    R::Error: From<String>,
{
    /// 预计算表切分键并重置结果状态。
    pub fn Open(&mut self, _context: &mut R::Context) -> Result<(), R::Error> {
        self.splitKeys = self.getSplitTableKeys()?;
        self.done = false;
        self.splitRegionResult = splitRegionResult::default();
        Ok(())
    }

    /// 一次性执行表 Region 切分，并把结果写入输出 chunk。
    pub fn Next(&mut self, context: &mut R::Context, chunk: &mut R::Chunk) -> Result<(), R::Error> {
        self.runtime.reset_chunk(chunk);
        if self.done {
            return Ok(());
        }
        self.done = true;
        self.splitTableRegion(context)?;
        appendSplitRegionResultToChunk(&mut self.runtime, chunk, self.splitRegionResult);
        Ok(())
    }

    /// 调用 runtime 切分表 Region，并按需等待 scatter。
    pub fn splitTableRegion(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        let start = Instant::now();
        let ids = match self.runtime.split_regions(
            context,
            self.splitKeys.clone(),
            self.runtime.table_id(),
        ) {
            Ok(ids) => ids,
            Err(error) => {
                self.runtime
                    .warn_split_failed(&self.runtime.table_name(), None, &error);
                Vec::new()
            }
        };
        self.splitRegionResult.splitRegions = ids.len() as i32;
        if !ids.is_empty() && self.runtime.wait_split_region_finish() {
            self.splitRegionResult.finishScatterNum =
                waitScatterRegionFinish(&mut self.runtime, context, start, ids, None);
        }
        Ok(())
    }

    fn physical_ids(&self) -> Result<Vec<i64>, R::Error> {
        selectedPhysicalIDs(&self.runtime, &self.partitionNames).map_err(Into::into)
    }

    /// 按值列表或上下界生成表切分键。
    pub fn getSplitTableKeys(&mut self) -> Result<Vec<Vec<u8>>, R::Error> {
        if self.valueLists.is_empty() {
            self.getSplitTableKeysFromBound()
        } else {
            self.getSplitTableKeysFromValueList()
        }
    }

    /// 对每个物理表按 valueLists 编码表 handle 键。
    pub fn getSplitTableKeysFromValueList(&mut self) -> Result<Vec<Vec<u8>>, R::Error> {
        let mut keys = Vec::new();
        for physical_id in self.physical_ids()? {
            keys = self.getSplitTablePhysicalKeysFromValueList(physical_id, keys)?;
        }
        Ok(keys)
    }

    /// 单个物理表：编码各 value 对应的表键。
    pub fn getSplitTablePhysicalKeysFromValueList(
        &mut self,
        physical_id: i64,
        mut keys: Vec<Vec<u8>>,
    ) -> Result<Vec<Vec<u8>>, R::Error> {
        for values in &self.valueLists {
            keys.push(self.runtime.encode_table_value_key(
                physical_id,
                &self.handleCols,
                values,
            )?);
        }
        Ok(keys)
    }

    /// 按 lower/upper/num 在每个物理表上均匀生成表切分键。
    pub fn getSplitTableKeysFromBound(&mut self) -> Result<Vec<Vec<u8>>, R::Error> {
        let mut keys = Vec::new();
        for physical_id in self.physical_ids()? {
            keys = self.getSplitTablePhysicalKeysFromBound(physical_id, keys)?;
        }
        Ok(keys)
    }

    /// 委托 runtime 按表上下界切分键。
    pub fn getSplitTablePhysicalKeysFromBound(
        &mut self,
        physical_id: i64,
        keys: Vec<Vec<u8>>,
    ) -> Result<Vec<Vec<u8>>, R::Error> {
        self.runtime.split_table_bound_keys(
            physical_id,
            &self.handleCols,
            &self.lower,
            &self.upper,
            self.num,
            keys,
        )
    }
}

/// 按分区名筛选物理表 ID；无分区则返回逻辑表 ID，名为空则返回全部物理 ID。
pub(crate) fn selectedPhysicalIDs<R: SplitRuntime>(
    runtime: &R,
    names: &[String],
) -> Result<Vec<i64>, String> {
    let partitions = runtime.partition_ids();
    if partitions.is_empty() {
        return Ok(vec![runtime.table_id()]);
    }
    if names.is_empty() {
        return Ok(partitions.into_iter().map(|(_, id)| id).collect());
    }
    names
        .iter()
        .map(|wanted| {
            partitions
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(wanted))
                .map(|(_, id)| *id)
                .ok_or_else(|| {
                    format!(
                        "unknown partition '{}' in table '{}'",
                        wanted,
                        runtime.table_name()
                    )
                })
        })
        .collect()
}

/// 在剩余超时内等待各 Region scatter 完成，返回成功完成的数量。
pub fn waitScatterRegionFinish<R: SplitRuntime>(
    runtime: &mut R,
    context: &mut R::Context,
    start_time: Instant,
    region_ids: Vec<u64>,
    index_name: Option<String>,
) -> i32 {
    let mut finished = 0;
    let table_name = runtime.table_name();
    for region_id in region_ids {
        // 上下文已取消时改用短退避，尽快结束等待
        let remaining = if isCtxDone(runtime, context) {
            checkScatterRegionFinishBackOff
        } else {
            runtime
                .wait_split_timeout()
                .saturating_sub(start_time.elapsed())
                .as_millis()
                .min(i32::MAX as u128) as i32
        };
        match runtime.wait_scatter_region_finish(context, region_id, remaining) {
            Ok(()) => finished += 1,
            Err(error) => {
                runtime.warn_scatter_failed(region_id, &table_name, index_name.as_deref(), &error)
            }
        }
    }
    finished
}

/// 把切分数量与 scatter 完成比例写入结果 chunk 的两列。
pub fn appendSplitRegionResultToChunk<R: SplitRuntime>(
    runtime: &mut R,
    chunk: &mut R::Chunk,
    result: splitRegionResult,
) {
    runtime.append_int64(chunk, 0, result.splitRegions as i64);
    let ratio = if result.finishScatterNum > 0 && result.splitRegions > 0 {
        result.finishScatterNum as f64 / result.splitRegions as f64
    } else {
        0.0
    };
    runtime.append_float64(chunk, 1, ratio);
}

/// 查询执行上下文是否已取消/结束。
pub fn isCtxDone<R: SplitRuntime>(runtime: &R, context: &R::Context) -> bool {
    runtime.context_done(context)
}

/// 加载物理表行键范围及公共索引范围上的 Region，去重后填充状态。
pub fn getPhysicalTableRegions<R: SplitRuntime>(
    runtime: &mut R,
    physical_table_id: i64,
    unique_region_ids: &mut HashSet<u64>,
) -> Result<Vec<regionMeta>, R::Error> {
    let (start, end) = runtime.table_handle_key_range(physical_table_id);
    let descriptors = runtime.load_regions(start, end)?;
    let mut regions = getRegionMeta(
        runtime,
        descriptors,
        unique_region_ids,
        physical_table_id,
        0,
        runtime.unsigned_int_handle(),
    )?;
    for index_id in runtime.public_index_ids() {
        let (start, end) = runtime.table_index_key_range(physical_table_id, index_id);
        let descriptors = runtime.load_regions(start, end)?;
        regions.extend(getRegionMeta(
            runtime,
            descriptors,
            unique_region_ids,
            physical_table_id,
            index_id,
            false,
        )?);
    }
    checkRegionsStatus(runtime, &mut regions)?;
    Ok(regions)
}

/// 加载指定物理表上某一索引键范围内的 Region。
pub fn getPhysicalIndexRegions<R: SplitRuntime>(
    runtime: &mut R,
    physical_table_id: i64,
    index_id: i64,
    unique_region_ids: &mut HashSet<u64>,
) -> Result<Vec<regionMeta>, R::Error> {
    let (start, end) = runtime.table_index_key_range(physical_table_id, index_id);
    let descriptors = runtime.load_regions(start, end)?;
    let mut regions = getRegionMeta(
        runtime,
        descriptors,
        unique_region_ids,
        physical_table_id,
        index_id,
        false,
    )?;
    checkRegionsStatus(runtime, &mut regions)?;
    Ok(regions)
}

/// 刷新各 Region 的 scatter 进行中标志。
pub fn checkRegionsStatus<R: SplitRuntime>(
    runtime: &mut R,
    regions: &mut [regionMeta],
) -> Result<(), R::Error> {
    for region in regions {
        region.scattering = runtime.region_is_scattering(region.region.id)?;
    }
    Ok(())
}

/// 用统一解码器把 Region 起止键格式化为可读字符串。
pub fn decodeRegionsKey(
    regions: &mut [regionMeta],
    table_prefix: Vec<u8>,
    record_prefix: Vec<u8>,
    index_prefix: Vec<u8>,
    physical_table_id: i64,
    index_id: i64,
    has_unsigned_int_handle: bool,
) {
    let decoder = regionKeyDecoder {
        physicalTableID: physical_table_id,
        tablePrefix: table_prefix,
        recordPrefix: record_prefix,
        indexPrefix: index_prefix,
        indexID: index_id,
        hasUnsignedIntHandle: has_unsigned_int_handle,
    };
    for region in regions {
        region.start = decoder.decodeRegionKey(&region.region.start_key);
        region.end = decoder.decodeRegionKey(&region.region.end_key);
    }
}

/// 按表/行/索引前缀把原始键解码为 `t_<id>_r_...` / `t_<id>_i_...` 形式。
pub struct regionKeyDecoder {
    pub physicalTableID: i64,
    pub tablePrefix: Vec<u8>,
    pub recordPrefix: Vec<u8>,
    pub indexPrefix: Vec<u8>,
    pub indexID: i64,
    pub hasUnsignedIntHandle: bool,
}

impl regionKeyDecoder {
    /// 按前缀优先级（索引 → 行记录 → 表 → 裸 `t`）解码单个键。
    pub fn decodeRegionKey(&self, key: &[u8]) -> String {
        // 索引键：前缀后的字节以十六进制展示
        if !self.indexPrefix.is_empty() && key.starts_with(&self.indexPrefix) {
            return format!(
                "t_{}_i_{}_{}",
                self.physicalTableID,
                self.indexID,
                hex(&key[self.indexPrefix.len()..])
            );
        }
        if !self.recordPrefix.is_empty() && key.starts_with(&self.recordPrefix) {
            let suffix = &key[self.recordPrefix.len()..];
            if suffix.is_empty() {
                return format!("t_{}_r", self.physicalTableID);
            }
            // 8 字节 handle：无符号整数主键时按 u64 展示，避免负数误解
            if suffix.len() == 8 {
                let signed = decode_comparable_i64(suffix).expect("eight byte handle");
                if self.hasUnsignedIntHandle {
                    return format!("t_{}_r_{}", self.physicalTableID, signed as u64);
                }
                return format!("t_{}_r_{signed}", self.physicalTableID);
            }
            return format!("t_{}_r_{}", self.physicalTableID, hex(suffix));
        }
        if !self.tablePrefix.is_empty() && key.starts_with(&self.tablePrefix) {
            let suffix = &key[self.tablePrefix.len()..];
            if !suffix.starts_with(b"_i") {
                return format!("t_{}_{}", self.physicalTableID, hex(suffix));
            }
            let encoded_index = &suffix[2..];
            if let Some(index_id) = decode_comparable_i64(encoded_index) {
                return format!(
                    "t_{}_i_{}_{}",
                    self.physicalTableID,
                    index_id,
                    hex(&encoded_index[8..])
                );
            }
            return format!("t_{}_i__{}", self.physicalTableID, hex(encoded_index));
        }
        if key.first() == Some(&b't') {
            let encoded_table = &key[1..];
            if let Some(table_id) = decode_comparable_i64(encoded_table) {
                return format!("t_{}_{}", table_id, hex(&encoded_table[8..]));
            }
            return format!("t_{}", hex(encoded_table));
        }
        hex(key)
    }
}

/// Go `codec.DecodeInt` 的定长整数部分：大端读取后恢复被翻转的符号位。
fn decode_comparable_i64(bytes: &[u8]) -> Option<i64> {
    let encoded = u64::from_be_bytes(bytes.get(..8)?.try_into().ok()?);
    Some((encoded ^ (1_u64 << 63)) as i64)
}

/// 字节切片转小写十六进制字符串。
fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

/// 去重描述符、拉取统计、解码可读键，组装 `regionMeta` 列表。
pub fn getRegionMeta<R: SplitRuntime>(
    runtime: &mut R,
    descriptors: Vec<RegionDescriptor>,
    unique_region_ids: &mut HashSet<u64>,
    physical_table_id: i64,
    index_id: i64,
    unsigned_int_handle: bool,
) -> Result<Vec<regionMeta>, R::Error> {
    let mut regions = descriptors
        .into_iter()
        .filter(|region| unique_region_ids.insert(region.id))
        .map(|region| regionMeta {
            leaderID: region.leader_id,
            storeID: region.store_id,
            physicalID: physical_table_id,
            region,
            ..regionMeta::default()
        })
        .collect::<Vec<_>>();
    getRegionInfo(runtime, &mut regions)?;
    decodeRegionsKey(
        &mut regions,
        runtime.table_prefix(physical_table_id),
        runtime.record_prefix(physical_table_id),
        if index_id == 0 {
            Vec::new()
        } else {
            runtime.index_prefix(physical_table_id, index_id)
        },
        physical_table_id,
        index_id,
        unsigned_int_handle,
    );
    Ok(regions)
}

/// 为各 Region 填充读写字节与近似 size/keys 统计。
pub fn getRegionInfo<R: SplitRuntime>(
    runtime: &mut R,
    regions: &mut [regionMeta],
) -> Result<(), R::Error> {
    for region in regions {
        if let Some(statistics) = runtime.region_statistics(region.region.id)? {
            region.writtenBytes = statistics.written_bytes;
            region.readBytes = statistics.read_bytes;
            region.approximateSize = statistics.approximate_size;
            region.approximateKeys = statistics.approximate_keys;
        }
    }
    Ok(())
}
