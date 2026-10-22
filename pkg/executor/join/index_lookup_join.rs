// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// Index LookUp Join（索引查找连接）执行器。
//
// 外表 outer worker 按批构造 lookup task；多个 inner worker 根据外表 key
// 构造索引 ranges、拉取内表行并构建 lookup map；主线程按外表顺序与 Joiner
// 产出连接结果。Index Lookup 指按索引定位内表行后再回表取完整行。

use crate::joiner::{Joiner, NaajType, Predicate, Row};
use crate::row_table_builder::Value;
use astersql_util_execdetails::execdetails::RuntimeStatsColl;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use astersql_executor_internal_exec::adaptive_limit_controller::{
    AdaptiveLimitController, AdaptiveLimitSnapshot,
};

/// 行级过滤谓词，与 Joiner 的 `Predicate` 同型。
pub type RowFilter = Predicate;

/// 单次索引 lookup 所需内容：外表 key、行及内表 key 列元数据。
#[derive(Clone, Debug, PartialEq)]
pub struct IndexJoinLookupContent {
    /// 外表连接键值。
    pub keys: Row,
    /// 对应外表完整行。
    pub row: Row,
    /// 内表 join key 列下标。
    pub key_columns: Vec<usize>,
    /// 内表 join key 列 ID（对齐 Go keyColumnIDs）。
    pub key_column_ids: Vec<i64>,
}

/// 根据外表 lookup 内容构造内表执行器并拉取匹配行。
pub trait IndexJoinExecutorBuilder: Send + Sync {
    fn build(&self, lookup_contents: &[IndexJoinLookupContent]) -> Result<Vec<Row>, String>;
}

/// 外表上下文：待连接行、key 列与过滤条件。
#[derive(Clone, Default)]
pub struct OuterCtx {
    /// 外表行缓冲。
    pub rows: Vec<Row>,
    /// 外表 join key 列下标。
    pub key_columns: Vec<usize>,
    /// 外表行过滤器（不满足则不参与 lookup）。
    pub filters: Vec<RowFilter>,
}

/// 内表上下文：索引执行器构建器与 key 列信息。
pub struct InnerCtx {
    /// 按 lookup key 构建内表扫描的 builder。
    pub builder: Box<dyn IndexJoinExecutorBuilder>,
    /// 内表 join key 列下标。
    pub key_columns: Vec<usize>,
    /// 内表 join key 列 ID。
    pub key_column_ids: Vec<i64>,
    /// 是否对所有 lookup key 启用 NULL-safe equal（`<=>`）语义。
    pub null_safe: bool,
}

/// 一批外表行的 lookup 任务：含匹配标记、内表结果与 key→行 哈希表。
#[derive(Clone, Debug, Default)]
pub struct LookUpJoinTask {
    /// 本批外表行。
    pub outer_rows: Vec<Row>,
    /// 各外表行是否通过 filter。
    pub outer_match: Vec<bool>,
    /// 去重后的 lookup 内容列表。
    pub lookup_contents: Vec<IndexJoinLookupContent>,
    /// 内表拉取到的行。
    pub inner_rows: Vec<Row>,
    /// 编码 key → 内表行列表，供主线程 join。
    pub lookup_map: HashMap<Vec<u8>, Vec<Row>>,
    /// 任务内游标（保留 Go 字段语义）。
    pub cursor: usize,
    /// inner worker 是否已完成该任务。
    pub done: bool,
}

impl LookUpJoinTask {
    /// 用外表行构造任务，默认全部 `outer_match=true`。
    pub fn new(outer_rows: Vec<Row>) -> Self {
        let length = outer_rows.len();
        Self {
            outer_rows,
            outer_match: vec![true; length],
            ..Self::default()
        }
    }
}

/// 外表 worker：按递增 batch 从 `OuterCtx` 切出 lookup 任务。
#[derive(Clone, Debug)]
pub struct OuterWorker {
    /// 每次 open 使用的初始批大小。
    initial_batch_size: usize,
    /// 当前批大小（会逐步加倍直至上限）。
    batch_size: usize,
    /// 批大小上限。
    max_batch_size: usize,
    /// 外表行读取游标。
    cursor: usize,
}
impl OuterWorker {
    /// 校验批大小后创建 worker。
    pub fn new(initial_batch_size: usize, max_batch_size: usize) -> Result<Self, String> {
        if initial_batch_size == 0 || max_batch_size == 0 {
            return Err("batch sizes must be positive".into());
        }
        Ok(Self {
            initial_batch_size: initial_batch_size.min(max_batch_size),
            batch_size: initial_batch_size.min(max_batch_size),
            max_batch_size,
            cursor: 0,
        })
    }
    /// 切下一批外表行并评估 filter，随后放大下一批 batch。
    pub fn build_task(&mut self, context: &OuterCtx) -> Result<Option<LookUpJoinTask>, String> {
        if self.cursor >= context.rows.len() {
            return Ok(None);
        }
        // Go buildTask 在计算 requiredRows 前先扩大当前 batch。
        self.increase_batch_size();
        let end = (self.cursor + self.batch_size).min(context.rows.len());
        let mut task = LookUpJoinTask::new(context.rows[self.cursor..end].to_vec());
        self.cursor = end;
        // 不满足 filter 的外表行标记为 false，后续不再构造 lookup key。
        for (index, row) in task.outer_rows.iter().enumerate() {
            task.outer_match[index] = filters_match(row, &context.filters)?;
        }
        Ok(Some(task))
    }
    /// 将 batch 加倍，不超过 `max_batch_size`。
    pub fn increase_batch_size(&mut self) {
        self.batch_size = (self.batch_size.saturating_mul(2)).min(self.max_batch_size);
    }
    /// 重置外表读取游标，供重新 open。
    pub fn reset(&mut self) {
        self.cursor = 0;
        self.batch_size = self.initial_batch_size;
    }
}

/// 内表 worker：构造 lookup key、拉取内表行并建 lookup map。
pub struct InnerWorker<'a> {
    /// 共享内表上下文。
    pub context: &'a InnerCtx,
}
impl InnerWorker<'_> {
    /// 从通过 filter 的外表行提取非 NULL key，排序去重后写入 `lookup_contents`。
    pub fn construct_lookup_content(
        &self,
        task: &mut LookUpJoinTask,
        outer_key_columns: &[usize],
    ) -> Result<(), String> {
        task.lookup_contents.clear();
        for (index, row) in task.outer_rows.iter().enumerate() {
            if !task.outer_match[index] {
                continue;
            }
            let mut keys = Vec::with_capacity(outer_key_columns.len());
            let mut valid = true;
            for column in outer_key_columns {
                let value = row
                    .get(*column)
                    .ok_or_else(|| format!("outer key column {column} is out of bounds"))?;
                if !self.context.null_safe && matches!(value, Value::Null) {
                    valid = false;
                    break;
                }
                keys.push(value.clone());
            }
            if valid {
                task.lookup_contents.push(IndexJoinLookupContent {
                    keys,
                    row: row.clone(),
                    key_columns: self.context.key_columns.clone(),
                    key_column_ids: self.context.key_column_ids.clone(),
                });
            }
        }
        // 排序去重，减少重复索引 ranges 扫描。
        task.lookup_contents
            .sort_by(|left, right| compare_row(&left.keys, &right.keys));
        task.lookup_contents
            .dedup_by(|left, right| compare_row(&left.keys, &right.keys).is_eq());
        Ok(())
    }
    /// 调用 builder 按 lookup 内容拉取内表行。
    pub fn fetch_inner_results(&self, task: &mut LookUpJoinTask) -> Result<(), String> {
        task.inner_rows = self.context.builder.build(&task.lookup_contents)?;
        Ok(())
    }
    /// 按内表 key 编码建立 `lookup_map`，供主线程按外表顺序探测。
    pub fn build_lookup_map(&self, task: &mut LookUpJoinTask) -> Result<(), String> {
        task.lookup_map.clear();
        for row in &task.inner_rows {
            let key = extract_key(row, &self.context.key_columns)?;
            if !self.context.null_safe && key.iter().any(|value| matches!(value, Value::Null)) {
                continue;
            }
            task.lookup_map
                .entry(encode_key(&key))
                .or_default()
                .push(row.clone());
        }
        Ok(())
    }
    /// 依次执行构造 key → 拉内表 → 建 map，并标记任务完成。
    pub fn handle_task(
        &self,
        task: &mut LookUpJoinTask,
        outer_key_columns: &[usize],
    ) -> Result<(), String> {
        self.construct_lookup_content(task, outer_key_columns)?;
        self.fetch_inner_results(task)?;
        self.build_lookup_map(task)?;
        task.done = true;
        Ok(())
    }
}

/// Index LookUp Join 执行器：外表分批 + 内表并发 lookup + Joiner 保序输出。
pub struct IndexLookUpJoin {
    /// 外表行与 key/filter 配置。
    pub outer_context: OuterCtx,
    /// 内表 builder 与 key 配置。
    pub inner_context: InnerCtx,
    /// 连接器：负责 key 匹配与未匹配补行。
    pub joiner: Joiner,
    /// 是否保持外表顺序（当前实现主线程本就按批顺序输出）。
    pub keep_outer_order: bool,
    /// 外表分批 worker。
    worker: OuterWorker,
    /// 已产出的结果行缓冲。
    output: Vec<Row>,
    /// 结果读取游标。
    cursor: usize,
    /// 是否已 open。
    opened: bool,
    /// 是否已 close；close 后必须显式 open，不得由 next 隐式重开。
    closed: bool,
    /// 当前 open 周期是否已执行，避免空结果被重复执行。
    executed: bool,
    /// 运行时统计。
    pub stats: IndexLookUpJoinRuntimeStats,
    /// True when the physical outer property requires ordered output.
    pub adaptive_limit_eligible: bool,
    /// Controller shared with the eligible outer lookup reader and LIMIT.
    pub adaptive_limit_controller: Option<Arc<AdaptiveLimitController>>,
}

impl IndexLookUpJoin {
    /// Bind worker candidate evidence to this statement and plan ID.
    pub fn with_runtime_stats(
        mut self,
        plan_id: i32,
        collection: Arc<Mutex<RuntimeStatsColl>>,
    ) -> Self {
        self.joiner = self.joiner.with_runtime_stats(plan_id, collection);
        self
    }

    /// 创建执行器并初始化外表 worker。
    pub fn new(
        outer_context: OuterCtx,
        inner_context: InnerCtx,
        joiner: Joiner,
        keep_outer_order: bool,
        initial_batch_size: usize,
        max_batch_size: usize,
    ) -> Result<Self, String> {
        Ok(Self {
            outer_context,
            inner_context,
            joiner,
            keep_outer_order,
            worker: OuterWorker::new(initial_batch_size, max_batch_size)?,
            output: Vec::new(),
            cursor: 0,
            opened: false,
            closed: false,
            executed: false,
            stats: IndexLookUpJoinRuntimeStats::default(),
            adaptive_limit_eligible: keep_outer_order,
            adaptive_limit_controller: None,
        })
    }
    /// 打开执行器：重置 worker 与输出缓冲。
    pub fn open(&mut self) -> Result<(), String> {
        self.joiner.reset_joined_rows();
        self.worker.reset();
        self.output.clear();
        self.cursor = 0;
        self.opened = true;
        self.closed = false;
        self.executed = false;
        Ok(())
    }
    /// 驱动全部外表任务：inner worker 建 lookup map 后按外表顺序 Joiner 出结果。
    fn execute(&mut self) -> Result<(), String> {
        let start = Instant::now();
        while let Some(mut task) = self.worker.build_task(&self.outer_context)? {
            let inner_start = Instant::now();
            {
                let worker = InnerWorker {
                    context: &self.inner_context,
                };
                worker.handle_task(&mut task, &self.outer_context.key_columns)?;
            }
            self.stats.inner_worker.total_time += inner_start.elapsed();
            self.stats.inner_worker.tasks += 1;
            // 按外表顺序探测：未过 filter / 无匹配时走 on_miss_match。
            for (index, outer) in task.outer_rows.iter().enumerate() {
                if !task.outer_match[index] {
                    self.joiner.on_miss_match(false, outer, &mut self.output);
                    continue;
                }
                let key = extract_key(outer, &self.outer_context.key_columns)?;
                let inners = task
                    .lookup_map
                    .get(&encode_key(&key))
                    .cloned()
                    .unwrap_or_default();
                let result = self.joiner.try_to_match_inners(
                    outer,
                    &inners,
                    &mut self.output,
                    NaajType::Unknown,
                )?;
                if !result.matched {
                    self.joiner
                        .on_miss_match(result.has_null, outer, &mut self.output);
                }
            }
        }
        self.stats.probe += start.elapsed();
        self.executed = true;
        Ok(())
    }
    /// 惰性执行并按 `required_rows` 切片返回结果。
    pub fn next(&mut self, required_rows: usize) -> Result<Vec<Row>, String> {
        if self.closed {
            return Err("cannot reopen closed index lookup join".into());
        }
        if !self.opened {
            self.open()?;
        }
        if required_rows == 0 {
            return Ok(Vec::new());
        }
        if !self.executed {
            self.execute()?;
        }
        if self.cursor >= self.output.len() {
            return Ok(Vec::new());
        }
        let end = (self.cursor + required_rows).min(self.output.len());
        let rows = self.output[self.cursor..end].to_vec();
        self.cursor = end;
        Ok(rows)
    }
    /// 关闭执行器并释放当前执行状态；保留输入以允许再次 open。
    pub fn close(&mut self) {
        self.joiner.register_joined_rows();
        if let Some(controller) = &self.adaptive_limit_controller {
            self.stats.adaptive_limit_snapshot = Some(controller.Snapshot());
        }
        self.output.clear();
        self.cursor = 0;
        self.opened = false;
        self.closed = true;
        self.executed = false;
    }
}

/// 内表 worker 各阶段耗时与任务计数。
#[derive(Clone, Debug, Default)]
pub struct InnerWorkerRuntimeStats {
    /// 处理全部任务的总耗时。
    pub total_time: Duration,
    /// 完成的任务数。
    pub tasks: u64,
    /// 构造 lookup key 耗时。
    pub construct: Duration,
    /// 拉取内表耗时。
    pub fetch: Duration,
    /// 建 lookup map 耗时。
    pub build: Duration,
    /// join 阶段耗时（保留 Go 字段）。
    pub join: Duration,
}
/// Index LookUp Join 整体运行时统计。
#[derive(Clone, Debug, Default)]
pub struct IndexLookUpJoinRuntimeStats {
    /// 内表并发度。
    pub concurrency: usize,
    /// 内表 worker 细分统计。
    pub inner_worker: InnerWorkerRuntimeStats,
    /// 主线程 probe/join 总耗时。
    pub probe: Duration,
    /// One executor-lifecycle snapshot; merges retain rather than add it.
    pub adaptive_limit_snapshot: Option<AdaptiveLimitSnapshot>,
}
impl IndexLookUpJoinRuntimeStats {
    /// 统计类型编号（对齐 Go TpIndexLookUpJoinRuntimeStats）。
    pub const TYPE: u8 = 6;
    /// 合并另一份统计；与 Go 一致，concurrency 保留接收者的值。
    pub fn merge(&mut self, other: &Self) {
        self.probe += other.probe;
        self.inner_worker.total_time += other.inner_worker.total_time;
        self.inner_worker.tasks += other.inner_worker.tasks;
        self.inner_worker.construct += other.inner_worker.construct;
        self.inner_worker.fetch += other.inner_worker.fetch;
        self.inner_worker.build += other.inner_worker.build;
        self.inner_worker.join += other.inner_worker.join;
        if self.adaptive_limit_snapshot.is_none() {
            self.adaptive_limit_snapshot = other.adaptive_limit_snapshot;
        }
    }
}
impl std::fmt::Display for IndexLookUpJoinRuntimeStats {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.inner_worker.total_time.is_zero() {
            write!(
                formatter,
                "inner:{{total:{:?}, concurrency:{}, task:{}, construct:{:?}, fetch:{:?}, build:{:?}",
                self.inner_worker.total_time,
                if self.concurrency > 0 {
                    self.concurrency.to_string()
                } else {
                    "OFF".into()
                },
                self.inner_worker.tasks,
                self.inner_worker.construct,
                self.inner_worker.fetch,
                self.inner_worker.build,
            )?;
            if !self.inner_worker.join.is_zero() {
                write!(formatter, ", join:{:?}", self.inner_worker.join)?;
            }
            write!(formatter, "}}")?;
        }
        if !self.probe.is_zero() {
            write!(formatter, ", probe:{:?}", self.probe)?;
        }
        if let Some(snapshot) = self.adaptive_limit_snapshot {
            if !self.inner_worker.total_time.is_zero() || !self.probe.is_zero() {
                write!(formatter, ", ")?;
            }
            write!(
                formatter,
                "adaptive:{{outer:{}/{}, lookup:{}/{}, outstanding:{}/{}, blocked:outer={:?},lookup={:?}}}",
                snapshot.outer_fetched,
                snapshot.outer_consumed,
                snapshot.lookup_handles,
                snapshot.lookup_rows,
                snapshot.outer_outstanding_at_stop,
                snapshot.lookup_outstanding_at_stop,
                snapshot.outer_admission_blocked,
                snapshot.lookup_admission_blocked,
            )?;
        }
        Ok(())
    }
}

/// 按列逐个比较两行 key 值。
pub fn compare_row(left: &[Value], right: &[Value]) -> std::cmp::Ordering {
    left.iter()
        .zip(right)
        .map(compare_value)
        .find(|order| !order.is_eq())
        .unwrap_or_else(|| left.len().cmp(&right.len()))
}
/// 按列下标从行中抽取 join key；越界报错。
pub fn extract_key(row: &Row, columns: &[usize]) -> Result<Row, String> {
    columns
        .iter()
        .map(|column| {
            row.get(*column)
                .cloned()
                .ok_or_else(|| format!("key column {column} is out of bounds"))
        })
        .collect()
}
/// 将 key 行编码为 lookup map 的字节键（Debug 格式占位，对齐迁移语义）。
pub fn encode_key(row: &[Value]) -> Vec<u8> {
    format!("{row:?}").into_bytes()
}
/// 所有 filter 均返回 `Some(true)` 才视为匹配。
fn filters_match(row: &Row, filters: &[RowFilter]) -> Result<bool, String> {
    for filter in filters {
        if filter(row)? != Some(true) {
            return Ok(false);
        }
    }
    Ok(true)
}
/// 单列 Value 比较：NULL 最小，同型按自然序，异型回退到 Debug 字符串。
fn compare_value(left: (&Value, &Value)) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match left {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Null, _) => Ordering::Less,
        (_, Value::Null) => Ordering::Greater,
        (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
        (Value::Int(a), Value::Int(b)) => a.cmp(b),
        (Value::UInt(a), Value::UInt(b)) => a.cmp(b),
        (Value::Float(a), Value::Float(b)) => a.total_cmp(b),
        (Value::Bytes(a), Value::Bytes(b)) => a.cmp(b),
        (Value::Text(a), Value::Text(b)) => a.cmp(b),
        (a, b) => format!("{a:?}").cmp(&format!("{b:?}")),
    }
}
