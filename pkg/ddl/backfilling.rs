// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// DDL 回填（backfilling）模块。
//
// 在线 DDL（如加索引、修改列类型）需要为表中已有的历史数据补写新的
// 索引记录或新格式的列数据，这个过程称为“回填”。回填时会把表的键
// 空间按 Region（分布式 KV 存储中的数据分片单位）切分成多个连续的
// 键区间（KeyRange），再把每个区间封装成回填任务分发给多个回填
// worker 并发处理。
//
// 本模块提供：
// - 回填类型枚举 `BackfillerType` 与回填器抽象 `Backfiller`；
// - 任务与结果结构 `ReorgBackfillTask` / `BackfillTaskContext` / `BackfillResult`；
// - 任务驱动循环 `handle_backfill_task`、区间校验与切分工具函数；
// - 乱序完成任务的进度归并器 `DoneTaskKeeper` 及行数统计器。

use std::collections::BTreeMap;
use std::fmt;

pub use astersql_kv::{Key as KvKey, KeyRange as KvKeyRange};

/// 原始字节形式的 KV 键（key）。存储层中表数据与索引数据都以
/// 有序字节串作为键，因此这里直接用字节向量表示。
pub type Key = Vec<u8>;

/// 回填器类型，对应不同的在线 DDL 回填场景。

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackfillerType {
    /// 添加索引：为已有行补写索引记录。
    AddIndex,
    /// 修改列：按新列定义重写已有行的列数据。
    UpdateColumn,
    /// 清理索引：删除无效或残留的索引记录。
    CleanupIndex,
    /// 合并临时索引：把 DDL 期间写入临时索引的增量数据合并回正式索引。
    MergeTemporaryIndex,
    /// 重组分区：把数据从旧分区搬迁到新分区布局。
    ReorganizePartition,
}

impl BackfillerType {
    /// 回填器类型的总数，与枚举成员个数保持一致。
    pub const COUNT: usize = 5;

    /// 返回类型的静态字符串名称，用于日志与监控指标。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AddIndex => "add index",
            Self::UpdateColumn => "update column",
            Self::CleanupIndex => "cleanup index",
            Self::MergeTemporaryIndex => "merge temporary index",
            Self::ReorganizePartition => "reorganize partition",
        }
    }
}

impl fmt::Display for BackfillerType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// 左闭右开的键区间 `[start_key, end_key)`，表示一段待回填的数据范围。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KeyRange {
    /// 区间起始键（包含）。
    pub start_key: Key,
    /// 区间结束键（不包含）。
    pub end_key: Key,
}

/// 一个重组（reorg）回填任务，描述某个物理表上一段键区间的回填工作。
///
/// “重组”指 DDL 过程中对已有数据的重新组织，如为历史行补建索引。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReorgBackfillTask {
    /// 物理表 ID。分区表的每个分区都有独立的物理表 ID。
    pub physical_table_id: i64,
    /// 任务编号，在同一个 DDL 作业中单调递增，用于按序归并进度。
    pub id: usize,
    /// 所属 DDL 作业（job）的 ID。
    pub job_id: i64,
    /// 本任务负责的起始键（包含）。
    pub start_key: Key,
    /// 本任务负责的结束键（不包含）。
    pub end_key: Key,
    /// 事务优先级，用于控制回填事务与用户事务的资源竞争。
    pub priority: i32,
}

/// 单批回填执行后的上下文信息，由 `Backfiller::backfill_data` 返回。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BackfillTaskContext {
    /// 下一批应从该键继续扫描，用于推进任务进度。
    pub next_key: Key,
    /// 是否已处理完本任务的整个区间。
    pub done: bool,
    /// 本批新写入（回填）的记录数。
    pub added_count: i64,
    /// 本批扫描过的记录数。
    pub scan_count: i64,
    /// 本批产生的警告信息，键为错误码，值为警告文本。
    pub warnings: BTreeMap<String, String>,
    /// 各错误码对应的警告出现次数。
    pub warning_counts: BTreeMap<String, i64>,
    /// 本批完成时的时间戳（TSO，全局授时服务分配的逻辑时间戳）。
    pub finish_ts: u64,
}

/// 整个回填任务执行完毕后的汇总结果。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BackfillResult {
    /// 对应的任务编号。
    pub task_id: usize,
    /// 任务实际推进到的键位置，出错时可据此断点续传。
    pub next_key: Key,
    /// 累计新写入的记录数。
    pub total_added_count: i64,
    /// 累计扫描的记录数。
    pub total_scan_count: i64,
    /// 累计的警告信息（按错误码去重）。
    pub warnings: BTreeMap<String, String>,
    /// 累计的各错误码警告次数。
    pub warning_counts: BTreeMap<String, i64>,
    /// 执行失败时的错误描述；`None` 表示成功。
    pub error: Option<String>,
}

/// 回填器抽象：由具体的回填实现（加索引、改列等）实现该 trait。
pub trait Backfiller {
    /// 处理任务中从 `task.start_key` 开始的一小批数据，返回执行上下文；
    /// 出错时返回错误描述字符串。
    fn backfill_data(&mut self, task: &ReorgBackfillTask) -> Result<BackfillTaskContext, String>;

    /// 上报监控指标（如已回填行数），默认实现为空操作。
    fn add_metric_info(&mut self, _added_count: i64) {}

    /// 回填器名称，用于日志与错误信息。
    fn name(&self) -> &'static str;
}

/// 把单批上下文中的警告及其计数合并进累计结果。
/// 警告文本按错误码保留首次值，次数则按错误码累加。
pub fn merge_warnings_and_counts(
    warnings: &mut BTreeMap<String, String>,
    warning_counts: &mut BTreeMap<String, i64>,
    task: &BackfillTaskContext,
) {
    for (code, warning) in &task.warnings {
        let count = task.warning_counts.get(code).copied().unwrap_or_default();
        if let Some(total) = warning_counts.get_mut(code) {
            *total += count;
        } else {
            warning_counts.insert(code.clone(), count);
            warnings.insert(code.clone(), warning.clone());
        }
    }
}

/// 驱动回填器处理完整个任务区间的主循环。
///
/// 反复调用 `backfill_data` 分批处理数据并累计统计信息，直到区间处理
/// 完毕、`runnable` 返回 false（worker 被要求停止）或发生错误为止。
/// 若某批未推进任何进度（`next_key` 未前移），视为异常并终止，避免死循环。
pub fn handle_backfill_task(
    backfiller: &mut impl Backfiller,
    task: &ReorgBackfillTask,
    runnable: impl Fn() -> bool,
) -> BackfillResult {
    let mut current = task.clone();
    let mut result = BackfillResult {
        task_id: task.id,
        next_key: task.start_key.clone(),
        ..BackfillResult::default()
    };

    // 循环分批处理，直到起始键追上结束键（区间处理完毕）。
    while current.start_key < current.end_key {
        // 每批开始前检查 worker 是否仍可运行（如 DDL 被取消或暂停）。
        if !runnable() {
            result.error = Some("backfill worker is no longer runnable".to_owned());
            break;
        }
        let context = match backfiller.backfill_data(&current) {
            Ok(context) => context,
            Err(error) => {
                result.error = Some(error);
                break;
            }
        };
        // 累计本批的统计数据与警告，并上报监控指标。
        result.total_added_count += context.added_count;
        result.total_scan_count += context.scan_count;
        merge_warnings_and_counts(&mut result.warnings, &mut result.warning_counts, &context);
        backfiller.add_metric_info(context.added_count);
        result.next_key = context.next_key.clone();
        if context.done {
            break;
        }
        // 进度未前移说明实现异常，立即报错以避免无限循环。
        if context.next_key <= current.start_key {
            result.error = Some(format!(
                "{} made no progress at key {:?}",
                backfiller.name(),
                current.start_key
            ));
            break;
        }
        current.start_key = context.next_key;
    }
    result
}

/// 区间校验相关错误类型，复用统一的共享错误定义。
pub type RangeError = astersql_util_dbterror::errors::SharedError;

/// 把键编码成十六进制字符串，便于在错误信息中展示二进制键。
fn encode_key(key: &[u8]) -> String {
    key.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// 构造“非法的 Region 切分区间”错误。
fn invalid_split_region_ranges(message: impl Into<String>) -> RangeError {
    astersql_util_dbterror::ErrInvalidSplitRegionRanges.GenWithStackByArgs(&[message.into().into()])
}

/// 校验并修正按 Region 切出的键区间列表。
///
/// Region 是分布式 KV 存储的数据分片，其边界可能超出目标表的键范围，
/// 因此这里把首个区间的起点收缩到 `start_key`、末尾区间的终点收缩到
/// `end_key`，并检查各区间非空且首尾相接（连续覆盖整个范围），
/// 任一条件不满足即返回错误。
pub fn validate_and_fill_ranges(
    ranges: &mut [KvKeyRange],
    start_key: &[u8],
    end_key: &[u8],
) -> Result<(), RangeError> {
    if ranges.is_empty() {
        return Err(invalid_split_region_ranges(format!(
            "cannot find region in range [{}, {}]",
            encode_key(start_key),
            encode_key(end_key)
        )));
    }

    let last = ranges.len() - 1;
    for index in 0..ranges.len() {
        // 首区间：起点早于目标范围时收缩到 start_key；晚于则说明开头有缺口。
        if index == 0 {
            let actual = ranges[index].StartKey.0.as_slice();
            if actual.is_empty() || actual < start_key {
                ranges[index].StartKey = KvKey(start_key.to_vec());
            } else if actual > start_key {
                return Err(invalid_split_region_ranges(format!(
                    "get empty range at the beginning of ranges, expected {}, but got {}",
                    encode_key(start_key),
                    encode_key(actual)
                )));
            }
        }

        // 末区间：终点超出目标范围（或为无界的空键）时收缩到 end_key。
        if index == last {
            let actual = ranges[index].EndKey.0.as_slice();
            if actual.is_empty() || actual > end_key {
                ranges[index].EndKey = KvKey(end_key.to_vec());
            }
        }

        // 中间区间不允许出现空键（空键表示无界，只有首尾区间可能出现）。
        if ranges[index].StartKey.0.is_empty() || ranges[index].EndKey.0.is_empty() {
            return Err(invalid_split_region_ranges(
                "get empty start/end key in the middle of ranges",
            ));
        }

        // 相邻区间必须首尾相接，否则说明范围覆盖不连续。
        if index > 0 && ranges[index - 1].EndKey != ranges[index].StartKey {
            return Err(invalid_split_region_ranges(format!(
                "ranges are not continuous, last end key {}, next start key {}",
                encode_key(&ranges[index - 1].EndKey.0),
                encode_key(&ranges[index].StartKey.0)
            )));
        }
    }
    Ok(())
}

/// 用有序的切分键把已有区间进一步细分成更小的子区间。
///
/// 常用于按已复制/导入的数据边界切分回填范围，使每个子任务粒度更均匀。
/// `split_keys` 需与区间同为升序；落在某区间内部的切分键会把该区间
/// 一分为二，落在区间之外的切分键会被跳过。
pub fn split_ranges_by_keys(ranges: &[KvKeyRange], split_keys: &[KvKey]) -> Vec<KvKeyRange> {
    if split_keys.is_empty() {
        return ranges.to_vec();
    }

    let mut result = Vec::with_capacity(ranges.len() + split_keys.len());
    let mut split_index = 0;
    for range in ranges {
        let mut start = range.StartKey.clone();
        loop {
            let Some(split) = split_keys.get(split_index) else {
                break;
            };
            // 切分键不超过当前起点：无效，跳过；
            // 落在区间内部：切出一个子区间并推进起点；
            // 达到或超过区间终点：留给后续区间处理。
            if split.Cmp(&start) <= 0 {
                split_index += 1;
            } else if split.Cmp(&range.EndKey) < 0 {
                split_index += 1;
                result.push(KvKeyRange {
                    StartKey: start,
                    EndKey: split.clone(),
                });
                start = split.clone();
            } else {
                break;
            }
        }
        // 追加当前区间剩余的尾部（可能就是完整的原区间）。
        result.push(KvKeyRange {
            StartKey: start,
            EndKey: range.EndKey.clone(),
        });
    }
    result
}

/// 把一批键区间转换成回填任务列表，任务编号从 `next_task_id` 起
/// 连续分配并回写推进该计数器。
pub fn get_batch_tasks(
    physical_table_id: i64,
    job_id: i64,
    ranges: &[KeyRange],
    next_task_id: &mut usize,
) -> Vec<ReorgBackfillTask> {
    ranges
        .iter()
        .map(|range| {
            let id = *next_task_id;
            *next_task_id += 1;
            ReorgBackfillTask {
                physical_table_id,
                id,
                job_id,
                start_key: range.start_key.clone(),
                end_key: range.end_key.clone(),
                priority: 0,
            }
        })
        .collect()
}

/// 在快照数据（某一时间点的一致性读视图）上按键序遍历指定范围内、
/// 具有给定前缀的键值对，对每条记录调用 `handle` 回调。
///
/// 回调返回 `Ok(false)` 表示提前终止遍历；返回值为下一次继续扫描的
/// 起始键（末尾追加 0 字节表示严格大于当前键的最小键）。
pub fn iterate_snapshot_keys<E>(
    rows: &[(Key, Vec<u8>)],
    key_prefix: &[u8],
    start_key: &[u8],
    end_key: &[u8],
    mut handle: impl FnMut(&[u8], &[u8]) -> Result<bool, E>,
) -> Result<Key, E> {
    let mut next_key = start_key.to_vec();
    // 只保留位于 [start_key, end_key) 且匹配前缀的记录。
    for (key, value) in rows.iter().filter(|(key, _)| {
        key.as_slice() >= start_key && key.as_slice() < end_key && key.starts_with(key_prefix)
    }) {
        next_key = key.clone();
        if !handle(key, value)? {
            return Ok(next_key);
        }
        // 追加 0 字节得到按字典序紧邻其后的键，避免重复处理当前键。
        next_key.push(0);
    }
    Ok(next_key)
}

/// 乱序完成任务的进度归并器。
///
/// 回填任务并发执行，完成顺序不确定，但持久化的断点进度必须按任务
/// 编号连续推进（否则宕机重启后会漏掉中间未完成的区间）。本结构缓存
/// 提前完成的任务进度，等编号连续时再统一推进 `next_key`。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DoneTaskKeeper {
    /// 已完成但尚不能推进的任务：任务编号 -> 该任务的结束键。
    done_task_next_key: BTreeMap<usize, KvKey>,
    /// 下一个期待完成的任务编号。
    current: usize,
    /// 当前可安全持久化的进度键。
    next_key: KvKey,
}

impl DoneTaskKeeper {
    /// 以给定起始键创建归并器，期待的首个任务编号为 0。
    pub fn new(start_key: KvKey) -> Self {
        Self {
            next_key: start_key,
            ..Self::default()
        }
    }

    /// 返回当前可安全持久化的进度键。
    pub fn next_key(&self) -> &[u8] {
        &self.next_key.0
    }

    /// 记录编号为 `done_task_id` 的任务已完成到 `next_key`。
    ///
    /// 若该任务不是当前期待的编号，先缓存；否则推进进度，并把缓存中
    /// 编号连续的后继任务一并消化掉。
    pub fn update_next_key(&mut self, done_task_id: usize, next_key: KvKey) {
        if done_task_id != self.current {
            self.done_task_next_key.insert(done_task_id, next_key);
            return;
        }
        self.current += 1;
        self.next_key = next_key;
        // 连续消化缓存中编号紧邻的已完成任务，尽可能推进进度。
        while let Some(next_key) = self.done_task_next_key.remove(&self.current) {
            self.current += 1;
            self.next_key = next_key;
        }
    }
}

/// 本地行数/字节数统计器，用于汇报回填吞吐进度。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalRowCountCollector {
    /// 已接收（待处理）的字节数。
    pub accepted_bytes: i64,
    /// 已处理完成的字节数。
    pub processed_bytes: i64,
    /// 已处理完成的行数。
    pub processed_rows: i64,
}

impl LocalRowCountCollector {
    /// 记录新接收了 `bytes` 字节的数据。
    pub fn accepted(&mut self, bytes: i64) {
        self.accepted_bytes += bytes;
    }

    /// 记录处理完成了 `bytes` 字节、`rows` 行数据。
    pub fn processed(&mut self, bytes: i64, rows: i64) {
        self.processed_bytes += bytes;
        self.processed_rows += rows;
    }
}
