// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 按 RangeProperty 将全局排序中间文件切分为 ranges group / range job / Region 分裂键。
//
// `RangeSplitter` 按 first_key 扫描属性条目，累计大小与键数，达到阈值时输出一组
// 仍活跃的 data/stat 文件及内部切分键。Region 是 TiKV 的数据分片单位；
// 预计算的 region split keys 用于后续 scatter/split，便于并行导入。

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use crate::{FilePair, MultipleFilesStat, RangeProperty, Result, Storage, decode_kvs};

/// 写步骤内存份额除数，用于 `CalRangeSize` 估算单 range 体积。
const writeStepMemShareCount: f64 = 6.5;
/// 估算 range job 内存时，每个键切片指针开销（近似 Go slice header）。
const SIZE_OF_SLICE: i64 = std::mem::size_of::<Vec<u8>>() as i64;

#[derive(Clone, Eq, PartialEq)]
/// 小根堆元素：某文件“耗尽”时对应的 last_key，用于延迟移出 active 集合。
struct ExhaustedHeapElem {
    key: Vec<u8>,
    data_file: String,
    stat_file: String,
}

/// 按 key（再按 data_file）逆序比较，使 BinaryHeap 表现为小根堆。
impl Ord for ExhaustedHeapElem {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .key
            .cmp(&self.key)
            .then_with(|| other.data_file.cmp(&self.data_file))
    }
}

impl PartialOrd for ExhaustedHeapElem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone)]
/// 打平后的属性条目：所属 multi-file 组/文件下标，及是否为该文件最后一段。
struct PropertyEntry {
    property: RangeProperty,
    group_index: usize,
    file_index: usize,
    last_for_file: bool,
}

/// 根据每核内存与 Region 分裂大小/键数，计算 range 目标字节数与键数。
///
/// 返回 `(range_size, range_keys)`；非法输入返回 `(0, 0)`。
pub fn CalRangeSize(memPerCore: i64, regionSplitSize: i64, regionSplitKeys: i64) -> (i64, i64) {
    if memPerCore <= 0 || regionSplitSize <= 0 || regionSplitKeys <= 0 {
        return (0, 0);
    }
    let share_size = (memPerCore as f64 / writeStepMemShareCount) as i64;
    let range_size = if share_size < regionSplitSize {
        let range_count = (regionSplitSize as f64 / share_size.max(1) as f64).ceil() as i64;
        regionSplitSize / range_count + 1
    } else {
        (share_size / regionSplitSize) * regionSplitSize
    };
    let average_key_size = regionSplitSize as f64 / regionSplitKeys as f64;
    (range_size, (range_size as f64 / average_key_size) as i64)
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 一次 `SplitOneRangesGroup` 的结果：组结束键、活跃文件与内部切分键。
pub struct SplitResult {
    /// 本组结束键（半开上界）；空表示已扫完全部属性（最后一组）。
    pub end_key_of_group: Vec<u8>,
    /// 本组仍需读取的 data 文件路径。
    pub data_files: Vec<String>,
    /// 与 data 对应的 stat 文件路径。
    pub stat_files: Vec<String>,
    /// 组内 range job 边界键（不含组端点）。
    pub interior_range_job_keys: Vec<Vec<u8>>,
    /// 组内 Region 分裂键（不含组端点）。
    pub interior_region_split_keys: Vec<Vec<u8>>,
}

/// 按属性顺序推进的切分器状态机。
pub struct RangeSplitter {
    ranges_group_size: i64,
    ranges_group_keys: i64,
    range_job_size: i64,
    range_job_key_count: i64,
    region_split_size: i64,
    region_split_key_count: i64,
    entries: Vec<PropertyEntry>,
    cursor: usize,
    multi_file_stat: Vec<MultipleFilesStat>,
    active_data_files: HashMap<String, (usize, usize)>,
    active_stat_files: HashMap<String, (usize, usize)>,
    current_group_size: i64,
    current_group_key_count: i64,
    current_range_job_size: i64,
    current_range_job_key_count: i64,
    record_range_job_after_next_property: bool,
    current_region_split_size: i64,
    current_region_split_key_count: i64,
    record_region_split_after_next_property: bool,
    range_job_keys: Vec<Vec<u8>>,
    region_split_keys: Vec<Vec<u8>>,
    last_data_file: String,
    last_stat_file: String,
    last_range_property: Option<RangeProperty>,
    last_range_property_exhausted_file: bool,
    will_exhaust: BinaryHeap<ExhaustedHeapElem>,
    closed: bool,
}

#[allow(clippy::too_many_arguments)]
/// 从 `MultipleFilesStat` 填充/派生属性，按 first_key 排序后构造切分器。
pub fn NewRangeSplitter(
    multi_file_stat: &[MultipleFilesStat],
    external_storage: &dyn Storage,
    ranges_group_size: i64,
    ranges_group_key_count: i64,
    range_job_size: i64,
    range_job_key_count: i64,
    region_split_size: i64,
    region_split_key_count: i64,
) -> Result<RangeSplitter> {
    let mut normalized = multi_file_stat.to_vec();
    let mut entries = Vec::new();
    for (group_index, group) in normalized.iter_mut().enumerate() {
        for (file_index, pair) in group.filenames.iter_mut().enumerate() {
            populate_properties(pair, external_storage)?;
            let count = pair.properties.len();
            for (property_index, property) in pair.properties.iter().cloned().enumerate() {
                entries.push(PropertyEntry {
                    property,
                    group_index,
                    file_index,
                    last_for_file: property_index + 1 == count,
                });
            }
        }
    }
    // 全局按 first_key，再按组/文件下标稳定排序，保证切分确定。
    entries.sort_by(|left, right| {
        left.property
            .first_key
            .cmp(&right.property.first_key)
            .then_with(|| left.group_index.cmp(&right.group_index))
            .then_with(|| left.file_index.cmp(&right.file_index))
    });
    Ok(RangeSplitter {
        ranges_group_size,
        ranges_group_keys: ranges_group_key_count,
        range_job_size,
        range_job_key_count,
        region_split_size,
        region_split_key_count,
        entries,
        cursor: 0,
        multi_file_stat: normalized,
        active_data_files: HashMap::new(),
        active_stat_files: HashMap::new(),
        current_group_size: 0,
        current_group_key_count: 0,
        current_range_job_size: 0,
        current_range_job_key_count: 0,
        record_range_job_after_next_property: false,
        current_region_split_size: 0,
        current_region_split_key_count: 0,
        record_region_split_after_next_property: false,
        range_job_keys: Vec::with_capacity(16),
        region_split_keys: Vec::with_capacity(16),
        last_data_file: String::new(),
        last_stat_file: String::new(),
        last_range_property: None,
        last_range_property_exhausted_file: false,
        will_exhaust: BinaryHeap::new(),
        closed: false,
    })
}

// 无独立 stat 边界编码时，从 data 文件为每条 KV 派生一个 RangeProperty（最细粒度）。
// Go's simplesst writer records a new RangeProperty boundary every
// `PropKeysDistance` keys (defaulting to a small constant); the tests in
// split_test.rs configure that distance down to 1 so each key gets its own
// boundary. The Rust production data files don't carry a separate stat
// encoding for these boundaries (see `MockExternalEngine`/`testutil.rs`), so
// this derives one property per KV pair directly from the data file,
// matching the finest (distance = 1) granularity Go's tests rely on.
/// 若 `properties` 为空，则解码 data 文件并为每个 KV 生成一段属性。
fn populate_properties(pair: &mut FilePair, storage: &dyn Storage) -> Result<()> {
    if !pair.properties.is_empty() {
        return Ok(());
    }
    let kvs = decode_kvs(&storage.read(&pair.data_file)?, 0)?;
    pair.properties = kvs
        .iter()
        .map(|kv| RangeProperty {
            first_key: kv.key.clone(),
            last_key: kv.key.clone(),
            size: kv.encoded_size() as u64,
            keys: 1,
        })
        .collect();
    Ok(())
}

impl RangeSplitter {
    /// 关闭切分器；之后再调用 `SplitOneRangesGroup` 返回 `Closed`。
    pub fn Close(&mut self) -> Result<()> {
        self.closed = true;
        Ok(())
    }

    /// 推进属性游标，直到凑满一个 ranges group 或耗尽全部条目。
    pub fn SplitOneRangesGroup(&mut self) -> Result<SplitResult> {
        if self.closed {
            return Err(crate::Error::Closed);
        }
        let mut exhausted_data_files = Vec::new();
        let mut exhausted_stat_files = Vec::new();
        let mut returned_files: Option<(Vec<String>, Vec<String>)> = None;
        let mut return_after_next_property = false;

        // 主循环：累计 size/keys，维护 active 文件与将耗尽堆，触发各级切分标志。
        while let Some(entry) = self.entries.get(self.cursor).cloned() {
            self.cursor += 1;
            let property = entry.property;
            self.current_group_size += property.size as i64;
            self.current_range_job_size += property.size as i64;
            self.current_region_split_size += property.size as i64;
            self.current_group_key_count += property.keys as i64;
            self.current_range_job_key_count += property.keys as i64;
            self.current_region_split_key_count += property.keys as i64;

            // 与 Go MergePropIter 的 close-reader flag 一致：进入下一属性时，
            // 若上一属性已是其文件末段，才把上一文件加入将耗尽堆。
            if self.last_range_property_exhausted_file {
                if let Some(last) = self.last_range_property.as_ref() {
                    self.will_exhaust.push(ExhaustedHeapElem {
                        key: last.last_key.clone(),
                        data_file: self.last_data_file.clone(),
                        stat_file: self.last_stat_file.clone(),
                    });
                }
            }

            let pair = &self.multi_file_stat[entry.group_index].filenames[entry.file_index];
            self.active_data_files.insert(
                pair.data_file.clone(),
                (entry.group_index, entry.file_index),
            );
            self.active_stat_files.insert(
                pair.stat_file.clone(),
                (entry.group_index, entry.file_index),
            );
            self.last_data_file.clone_from(&pair.data_file);
            self.last_stat_file.clone_from(&pair.stat_file);
            self.last_range_property = Some(property.clone());
            self.last_range_property_exhausted_file = entry.last_for_file;

            while self
                .will_exhaust
                .peek()
                // 弹出已真正耗尽（last_key 早于当前 first_key）的文件路径。
                .is_some_and(|item| item.key < property.first_key)
            {
                let item = self.will_exhaust.pop().unwrap();
                exhausted_data_files.push(item.data_file);
                exhausted_stat_files.push(item.stat_file);
            }

            // 上一段已达 group 阈值：在下一段 first_key 处返回，作为组上界。
            if return_after_next_property {
                for path in exhausted_data_files {
                    self.active_data_files.remove(&path);
                }
                for path in exhausted_stat_files {
                    self.active_stat_files.remove(&path);
                }
                let (data_files, stat_files) = returned_files.unwrap_or_default();
                return Ok(SplitResult {
                    end_key_of_group: property.first_key,
                    data_files,
                    stat_files,
                    interior_range_job_keys: self.take_range_job_keys(),
                    interior_region_split_keys: self.take_region_split_keys(),
                });
            }
            if self.record_range_job_after_next_property {
                self.range_job_keys.push(property.first_key.clone());
                self.record_range_job_after_next_property = false;
            }
            if self.record_region_split_after_next_property {
                self.region_split_keys.push(property.first_key.clone());
                self.record_region_split_after_next_property = false;
            }

            // 估算 range job 内存（数据 + 键切片开销），超限则标记下一属性记录边界。
            let range_memory = self.current_range_job_size.saturating_add(
                self.current_range_job_key_count
                    .saturating_mul(SIZE_OF_SLICE * 2),
            );
            if range_memory >= self.range_job_size
                || self.current_range_job_key_count >= self.range_job_key_count
            {
                self.current_range_job_size = 0;
                self.current_range_job_key_count = 0;
                self.record_range_job_after_next_property = true;
            }
            if self.current_region_split_size >= self.region_split_size
                || self.current_region_split_key_count >= self.region_split_key_count
            {
                self.current_region_split_size = 0;
                self.current_region_split_key_count = 0;
                self.record_region_split_after_next_property = true;
            }
            // ranges group 体积/键数达标：快照 active 文件，下一属性处返回。
            if self.current_group_size >= self.ranges_group_size
                || self.current_group_key_count >= self.ranges_group_keys
            {
                returned_files = Some(self.clone_active_files());
                self.current_group_size = 0;
                self.current_group_key_count = 0;
                return_after_next_property = true;
            }
        }

        // 属性耗尽：返回剩余 active 文件，end_key 为空表示结束。
        let (data_files, stat_files) = self.clone_active_files();
        self.active_data_files.clear();
        self.active_stat_files.clear();
        Ok(SplitResult {
            end_key_of_group: Vec::new(),
            data_files,
            stat_files,
            interior_range_job_keys: self.take_range_job_keys(),
            interior_region_split_keys: self.take_region_split_keys(),
        })
    }

    /// 按插入顺序（组/文件下标）克隆当前 active 的 data/stat 路径。
    fn clone_active_files(&self) -> (Vec<String>, Vec<String>) {
        let mut data_files: Vec<_> = self.active_data_files.keys().cloned().collect();
        data_files.sort_by_key(|path| self.active_data_files[path]);
        let mut stat_files: Vec<_> = self.active_stat_files.keys().cloned().collect();
        stat_files.sort_by_key(|path| self.active_stat_files[path]);
        (data_files, stat_files)
    }

    /// 取出并清空已记录的 range job 边界键。
    fn take_range_job_keys(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.range_job_keys)
    }

    /// 取出并清空已记录的 Region 分裂键。
    fn take_region_split_keys(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.region_split_keys)
    }
}
