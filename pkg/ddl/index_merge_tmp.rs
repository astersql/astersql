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

// 临时索引合并模块（index merge tmp）。
//
// 在线 DDL（如 ADD INDEX）过程中，为了不阻塞前台事务写入，新产生的索引
// 变更会先写入一份"临时索引"（temporary index，键前缀与正式索引不同）。
// 回填（backfill）完成历史数据后，需要把临时索引中积累的增量记录合并回
// 正式索引，这一步称为"临时索引合并"。本模块提供合并阶段所需的基础能力：
//
// - `TemporaryIndexRecord`：一条从临时索引解析出的记录；
// - `check_temporary_index_key` / `batch_check_temporary_unique_key`：
//   唯一索引合并前的冲突检查（判断跳过或报重复键）；
// - `TemporaryIndexBuffers` / `fetch_temporary_index_values`：按键区间
//   分批拉取待合并记录；
// - `find_index_info_by_decoding_key` / `decode_temporary_index_handle`：
//   从编码后的键值中解出索引 ID 与行句柄（handle，行的唯一定位标识）。

use crate::backfilling::Key;
use std::collections::BTreeMap;

/// 从临时索引中解析出的一条索引记录。
///
/// 合并流程会把该记录从临时索引键空间搬移到正式索引键空间。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TemporaryIndexRecord {
    /// 临时索引中的键（带临时索引前缀）。
    pub temporary_key: Key,
    /// 对应正式索引中的键（合并的目标位置）。
    pub original_key: Key,
    /// 索引值（编码后的字节序列）。
    pub value: Vec<u8>,
    /// 行句柄（handle）：标识该索引项指向的表行，用于唯一性冲突判断。
    pub handle: Vec<u8>,
    /// 值是否编码为唯一（distinct）索引项；唯一索引中的 NULL 值为非 distinct。
    pub distinct: bool,
    /// 是否为删除操作（临时索引里记录的可能是删除标记而非插入）。
    pub delete: bool,
    /// 是否在合并时跳过该记录（由冲突检查阶段填写）。
    pub skip: bool,
}

/// 正式索引中已有值及删除冲突检查所需的行存活状态。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OriginalIndexValue {
    pub value: Vec<u8>,
    pub distinct: bool,
    pub row_exists: Result<bool, MergeError>,
}

impl OriginalIndexValue {
    pub fn distinct(value: &[u8], row_exists: bool) -> Self {
        Self {
            value: value.to_vec(),
            distinct: true,
            row_exists: Ok(row_exists),
        }
    }

    pub fn non_distinct(value: &[u8]) -> Self {
        Self {
            value: value.to_vec(),
            distinct: false,
            row_exists: Ok(false),
        }
    }

    pub fn distinct_lookup_error(value: &[u8], error: MergeError) -> Self {
        Self {
            value: value.to_vec(),
            distinct: true,
            row_exists: Err(error),
        }
    }
}
/// 临时索引合并过程中可能出现的错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MergeError {
    /// 唯一索引冲突：同一索引键指向了不同的行句柄。
    DuplicateKey,
    /// 键区间或参数非法（如 start >= end、批大小为 0）。
    InvalidKey,
    /// 键值解码失败（长度不足或索引 ID 不匹配）。
    Decode,
    /// 合并未取得进展。
    NoProgress,
}

/// 检查一条临时索引记录与正式索引中已有值的关系。
///
/// 与 Go `checkTempIndexKey` 一致，返回 `Ok(true)` 表示删除记录精确匹配正式
/// 索引值，调用方应从批内冲突集合移除该键；是否跳过合并写入记录在 `skip`
/// 字段中。`Err(DuplicateKey)` 表示唯一索引冲突。
pub fn check_temporary_index_key(
    record: &mut TemporaryIndexRecord,
    original: &OriginalIndexValue,
) -> Result<bool, MergeError> {
    if !record.delete {
        if record.distinct && original.value != record.value {
            return Err(MergeError::DuplicateKey);
        }
        record.skip = true;
        return Ok(false);
    }
    if !original.distinct {
        return Ok(false);
    }
    if record.handle != original.value {
        record.skip = original.row_exists.clone()?;
        return Ok(false);
    }
    Ok(true)
}
/// 批量对唯一索引记录做冲突检查，并把结果写入各记录的 `skip` 字段。
///
/// `original` 为正式索引当前的键值快照（BTreeMap 模拟有序存储引擎）。
/// 任何一条记录检查出重复键都会使整批返回错误。
pub fn batch_check_temporary_unique_key(
    records: &mut [TemporaryIndexRecord],
    original: &BTreeMap<Key, OriginalIndexValue>,
    unique: bool,
) -> Result<(), MergeError> {
    if !unique {
        return Ok(());
    }
    let mut batch_values = original.clone();
    for record in records {
        if let Some(value) = batch_values.get(&record.original_key) {
            if check_temporary_index_key(record, value)? {
                batch_values.remove(&record.original_key);
            }
        } else if record.distinct {
            batch_values.insert(
                record.original_key.clone(),
                OriginalIndexValue::distinct(&record.value, true),
            );
        }
    }
    Ok(())
}

/// 一个批次内待合并记录的缓冲区，可跨批次复用以减少内存分配。
#[derive(Clone, Debug, Default)]
pub struct TemporaryIndexBuffers {
    /// 本批次拉取到的完整记录。
    pub records: Vec<TemporaryIndexRecord>,
    /// 与 `records` 一一对应的正式索引键，便于批量查询正式索引。
    pub original_keys: Vec<Key>,
    /// 与 `records` 一一对应的临时索引键，便于合并后清理临时索引。
    pub temporary_keys: Vec<Key>,
}
/// `TemporaryIndexBuffers` 的轻量操作方法。
///
/// 这些方法只负责维护批处理阶段的三组并行数组，不承担额外业务逻辑。
impl TemporaryIndexBuffers {
    /// 按预期批大小预分配缓冲区容量。
    pub fn with_capacity(size: usize) -> Self {
        Self {
            records: Vec::with_capacity(size),
            original_keys: Vec::with_capacity(size),
            temporary_keys: Vec::with_capacity(size),
        }
    }
    /// 清空缓冲区内容但保留已分配容量，供下一批次复用。
    pub fn reset(&mut self) {
        self.records.clear();
        self.original_keys.clear();
        self.temporary_keys.clear();
    }
    /// 追加一条记录，同时登记其正式键与临时键。
    pub fn add(&mut self, record: TemporaryIndexRecord) {
        // 三个向量按相同顺序增长，依赖位置对应关系做后续批处理。
        self.original_keys.push(record.original_key.clone());
        self.temporary_keys.push(record.temporary_key.clone());
        self.records.push(record);
    }
}

/// 一次批量拉取的结果统计。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TemporaryIndexResult {
    /// 下一批次的起始键（本批最后一个键加 0 字节后缀，即其后继键）。
    pub next_key: Key,
    /// 本批扫描到的记录总数。
    pub scan_count: usize,
    /// 本批需要真正写入正式索引的记录数（去掉被标记 skip 的）。
    pub add_count: usize,
    /// 是否已扫完整个区间（拉取数量不足一批即视为结束）。
    pub done: bool,
}
/// 从临时索引记录集中按键区间 `[start, end)` 拉取一批待合并记录。
///
/// 模拟存储引擎的范围扫描（range scan）：过滤落在区间内的记录，最多取
/// `batch_size` 条填入 `buffers`，并返回本批统计与下一批起点。
pub fn fetch_temporary_index_values(
    records: &[TemporaryIndexRecord],
    start: &[u8],
    end: &[u8],
    batch_size: usize,
    buffers: &mut TemporaryIndexBuffers,
) -> Result<TemporaryIndexResult, MergeError> {
    // 参数校验：空区间或零批大小都无法取得进展，直接报错。
    if start >= end || batch_size == 0 {
        return Err(MergeError::InvalidKey);
    }
    buffers.reset();
    // 存储引擎按键有序扫描；输入切片本身不要求预排序。
    let mut selected: Vec<&TemporaryIndexRecord> = records
        .iter()
        .filter(|record| {
            record.temporary_key.as_slice() >= start && record.temporary_key.as_slice() < end
        })
        .collect();
    selected.sort_by(|left, right| left.temporary_key.cmp(&right.temporary_key));
    for record in selected.into_iter().take(batch_size) {
        buffers.add(record.clone());
    }
    // 计算下一批起始键：取本批最后一个临时键并追加 0x00 字节得到其
    // 字典序后继；若本批为空则直接推进到区间终点 end。
    let next = buffers
        .temporary_keys
        .last()
        .map(|key| {
            let mut key = key.clone();
            key.push(0);
            key
        })
        .unwrap_or_else(|| {
            let mut key = end.to_vec();
            key.push(0);
            key
        });
    Ok(TemporaryIndexResult {
        next_key: next,
        scan_count: buffers.records.len(),
        // Go 的 fetchTempIndexVals 只负责扫描，写入计数由事务合并阶段填写。
        add_count: 0,
        // Go 会再发起一次空扫描确认区间结束。
        done: buffers.records.is_empty(),
    })
}
/// 从编码后的索引键中解出索引 ID，并校验其属于给定的索引 ID 列表。
///
/// tablecodec 键头 `t{tableID}_i{indexID}` 的索引 ID 使用符号位翻转的大端
/// 编码，临时索引高位标记需先掩掉；键头非法或 ID 不在列表中均为解码错误。
pub fn find_index_info_by_decoding_key(index_ids: &[i64], key: &[u8]) -> Result<i64, MergeError> {
    const SIGN_MASK: u64 = 1_u64 << 63;
    const INDEX_ID_MASK: i64 = 0x0000_ffff_ffff_ffff;
    const INDEX_SEPARATOR_START: usize = 1 + 8;
    const INDEX_ID_START: usize = INDEX_SEPARATOR_START + 2;
    if key.first() != Some(&b't') || key.get(INDEX_SEPARATOR_START..INDEX_ID_START) != Some(b"_i") {
        return Err(MergeError::Decode);
    }
    let bytes: [u8; 8] = key
        .get(INDEX_ID_START..INDEX_ID_START + 8)
        .ok_or(MergeError::Decode)?
        .try_into()
        .map_err(|_| MergeError::Decode)?;
    let id = ((u64::from_be_bytes(bytes) ^ SIGN_MASK) as i64) & INDEX_ID_MASK;
    // 索引 ID 必须在待合并的索引列表内才有效。
    index_ids
        .contains(&id)
        .then_some(id)
        .ok_or(MergeError::Decode)
}
/// 解出临时索引记录对应的行句柄（handle）。
///
/// 插入记录的句柄存放在值（`value_handle`）中；删除记录没有值，
/// 句柄编码在键的末尾 8 字节里。
pub fn decode_temporary_index_handle(
    key: &[u8],
    value_handle: Option<&[u8]>,
    delete: bool,
) -> Result<Vec<u8>, MergeError> {
    // 非删除记录：句柄直接来自值部分。
    if !delete {
        return value_handle
            .map(ToOwned::to_owned)
            .ok_or(MergeError::Decode);
    }
    // 删除记录：从键末尾 8 字节截取句柄。
    key.get(key.len().checked_sub(8).ok_or(MergeError::Decode)?..)
        .map(ToOwned::to_owned)
        .ok_or(MergeError::Decode)
}
