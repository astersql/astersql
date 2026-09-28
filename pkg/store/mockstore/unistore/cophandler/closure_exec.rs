// Copyright 2026 AsterSQL.
// Copyright 2019-present PingCAP, Inc.
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

// Closure 风格 Coprocessor 执行器（对应 Go closure executor）。
//
// 在确认执行计划为可支持的线性形态后，以扫描/处理/收尾闭包链运行，
// 避免逐行 trait 对象开销；并提供 Count/Scan/Selection/TopN/HashCount 等处理器。

use crate::cop_handler::{
    Chunk, CopError, Datum, Executor, Expr, KeyRange, KvPair, KvReader, LockInfo, Row, append_row,
    check_lock,
};
use crate::mpp_exec::{ExecutionOutput, execute_executor};
use crate::topn::TopNHeap;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// 单个 Chunk 最多容纳的行数。
pub const CHUNK_MAX_ROWS: usize = 1024;

/// 扫描类型：表扫描或索引扫描。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanType {
    /// 表（行）扫描。
    Table,
    /// 索引扫描。
    Index,
}

/// 单个算子执行明细：耗时、迭代次数与产出行数。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExecDetail {
    /// 处理耗时累计。
    pub time_processed: Duration,
    /// Next/迭代次数。
    pub num_iterations: u64,
    /// 产出行数累计。
    pub num_produced_rows: u64,
}

impl ExecDetail {
    /// 用一次调用的起点与产出行数更新明细。
    pub fn update(&mut self, started: Instant, produced: usize) {
        self.time_processed += started.elapsed();
        self.num_iterations += 1;
        self.num_produced_rows += produced as u64;
    }
}

/// Optimized closure executor. It validates the same supported linear shapes
/// as Go, then runs scan/process/finish without constructing row-at-a-time
/// trait objects.
///
/// 优化闭包执行器：校验与 Go 相同的线性计划形态后执行扫描。
pub struct ClosureExecutor<'a> {
    /// KV 读取器。
    reader: &'a dyn KvReader,
    /// 扫描键范围列表。
    ranges: Vec<KeyRange>,
    /// 事务 start_ts（MVCC 快照）。
    start_ts: u64,
    /// 已解析锁的 start_ts 列表，检查锁时可跳过。
    resolved_locks: Vec<u64>,
    /// 执行计划根节点。
    root: Executor,
    /// 是否收集各 range 计数/NDV。
    collect_range_counts: bool,
    /// 各算子执行明细。
    pub details: Vec<ExecDetail>,
}

impl<'a> ClosureExecutor<'a> {
    /// 构造执行器；计划形态不受支持时返回错误。
    pub fn new(
        reader: &'a dyn KvReader,
        ranges: Vec<KeyRange>,
        start_ts: u64,
        resolved_locks: Vec<u64>,
        root: Executor,
        collect_range_counts: bool,
    ) -> Result<Self, CopError> {
        if !closure_supported(&root) {
            return Err(CopError::Unsupported("closure executor shape"));
        }
        Ok(Self {
            reader,
            ranges,
            start_ts,
            resolved_locks,
            root,
            collect_range_counts,
            details: Vec::new(),
        })
    }

    /// 执行整棵计划树并记录根级 ExecDetail。
    pub fn execute(&mut self) -> Result<ExecutionOutput, CopError> {
        let started = Instant::now();
        let mut output = execute_executor(self.reader, &self.ranges, self.start_ts, &self.root)?;
        if !self.collect_range_counts {
            output.range_counts.clear();
            output.ndvs.clear();
        }
        self.details.push(ExecDetail {
            time_processed: started.elapsed(),
            num_iterations: 1,
            num_produced_rows: output.rows.len() as u64,
        });
        Ok(output)
    }

    /// 判断范围是否为点查（end == prefix_next(start)）。
    pub fn is_point_get_range(range: &KeyRange) -> bool {
        prefix_next(&range.start) == range.end
    }

    /// 检查 ranges 内未 resolved 的锁，命中则返回 Locked。
    pub fn check_range_locks(&self, locks: &[LockInfo]) -> Result<(), CopError> {
        for range in &self.ranges {
            for lock in locks.iter().filter(|lock| {
                lock.key >= range.start && (range.end.is_empty() || lock.key < range.end)
            }) {
                check_lock(lock, &lock.key, self.start_ts, &self.resolved_locks)?;
            }
        }
        Ok(())
    }
}

/// 递归判断是否为 closure 支持的线性形态（Scan + Selection/Limit/TopN/Agg）。
fn closure_supported(executor: &Executor) -> bool {
    match executor {
        Executor::TableScan { .. } | Executor::IndexScan { .. } => true,
        Executor::Selection { child, .. }
        | Executor::Limit { child, .. }
        | Executor::TopN { child, .. }
        | Executor::Aggregation { child, .. } => closure_supported(child),
        _ => false,
    }
}

/// 闭包处理器：逐 KV 处理，结束时产出行集。
pub trait ClosureProcessor {
    /// 是否可跳过 value 解码（如 COUNT(*)）。
    fn skip_value(&self) -> bool {
        false
    }
    /// 处理一条 KV。
    fn process(&mut self, pair: &KvPair) -> Result<(), CopError>;
    /// 扫描结束后产出结果行。
    fn finish(&mut self) -> Result<Vec<Row>, CopError>;
}

/// COUNT(*)：只计数，不读 value。
#[derive(Default)]
pub struct CountStarProcessor {
    count: u64,
}
impl ClosureProcessor for CountStarProcessor {
    fn skip_value(&self) -> bool {
        true
    }
    fn process(&mut self, _pair: &KvPair) -> Result<(), CopError> {
        self.count += 1;
        Ok(())
    }
    fn finish(&mut self) -> Result<Vec<Row>, CopError> {
        Ok(vec![vec![Datum::Uint(self.count)]])
    }
}

/// COUNT(column)：跳过 NULL 后计数。
pub struct CountColumnProcessor {
    /// 列偏移。
    offset: usize,
    count: u64,
}
impl CountColumnProcessor {
    /// 指定列偏移构造。
    pub fn new(offset: usize) -> Self {
        Self { offset, count: 0 }
    }
}
impl ClosureProcessor for CountColumnProcessor {
    fn process(&mut self, pair: &KvPair) -> Result<(), CopError> {
        if !matches!(
            pair.value
                .get(self.offset)
                .ok_or(CopError::ColumnOffset(self.offset))?,
            Datum::Null
        ) {
            self.count += 1;
        }
        Ok(())
    }
    fn finish(&mut self) -> Result<Vec<Row>, CopError> {
        Ok(vec![vec![Datum::Uint(self.count)]])
    }
}

/// 表扫描处理器：按列投影收集行。
pub struct TableScanProcessor {
    columns: Vec<usize>,
    rows: Vec<Row>,
}
impl TableScanProcessor {
    /// 指定投影列构造。
    pub fn new(columns: Vec<usize>) -> Self {
        Self {
            columns,
            rows: Vec::new(),
        }
    }
}
impl ClosureProcessor for TableScanProcessor {
    fn process(&mut self, pair: &KvPair) -> Result<(), CopError> {
        self.rows.push(
            self.columns
                .iter()
                .map(|offset| {
                    pair.value
                        .get(*offset)
                        .cloned()
                        .ok_or(CopError::ColumnOffset(*offset))
                })
                .collect::<Result<_, _>>()?,
        );
        Ok(())
    }
    fn finish(&mut self) -> Result<Vec<Row>, CopError> {
        Ok(std::mem::take(&mut self.rows))
    }
}

/// Selection 包装：条件为真时才交给子处理器。
pub struct SelectionProcessor<P> {
    child: P,
    condition: Expr,
}
impl<P> SelectionProcessor<P> {
    /// 包装子处理器与过滤表达式。
    pub fn new(child: P, condition: Expr) -> Self {
        Self { child, condition }
    }
}
impl<P: ClosureProcessor> ClosureProcessor for SelectionProcessor<P> {
    fn process(&mut self, pair: &KvPair) -> Result<(), CopError> {
        if self.condition.eval(&pair.value)?.truthy() {
            self.child.process(pair)?;
        }
        Ok(())
    }
    fn finish(&mut self) -> Result<Vec<Row>, CopError> {
        self.child.finish()
    }
}

/// TopN 处理器：借助 TopNHeap 维护前 N 行。
pub struct TopNProcessor {
    heap: TopNHeap,
}
impl TopNProcessor {
    /// 使用已配置的堆构造。
    pub fn new(heap: TopNHeap) -> Self {
        Self { heap }
    }
}
impl ClosureProcessor for TopNProcessor {
    fn process(&mut self, pair: &KvPair) -> Result<(), CopError> {
        self.heap.add_data_row(pair.value.clone())?;
        Ok(())
    }
    fn finish(&mut self) -> Result<Vec<Row>, CopError> {
        // 取出堆内容时用空堆替换，避免 move 后留下半初始化状态。
        let replacement = TopNHeap::new(
            self.heap.total_count,
            self.heap.sorter.order_by_items.clone(),
        );
        std::mem::replace(&mut self.heap, replacement).into_sorted_rows()
    }
}

/// 按 group-by 表达式做哈希 COUNT。
#[derive(Default)]
pub struct HashCountProcessor {
    group_by: Vec<Expr>,
    counts: BTreeMap<Vec<Datum>, u64>,
}
impl HashCountProcessor {
    /// 指定分组表达式构造。
    pub fn new(group_by: Vec<Expr>) -> Self {
        Self {
            group_by,
            counts: BTreeMap::new(),
        }
    }
}
impl ClosureProcessor for HashCountProcessor {
    fn process(&mut self, pair: &KvPair) -> Result<(), CopError> {
        let key = self
            .group_by
            .iter()
            .map(|expr| expr.eval(&pair.value))
            .collect::<Result<Vec<_>, _>>()?;
        *self.counts.entry(key).or_default() += 1;
        Ok(())
    }
    fn finish(&mut self) -> Result<Vec<Row>, CopError> {
        Ok(std::mem::take(&mut self.counts)
            .into_iter()
            .map(|(mut key, count)| {
                key.push(Datum::Uint(count));
                key
            })
            .collect())
    }
}

/// 扫描 ranges 并驱动处理器 process/finish。
pub fn run_processor(
    reader: &dyn KvReader,
    ranges: &[KeyRange],
    start_ts: u64,
    descending: bool,
    processor: &mut dyn ClosureProcessor,
) -> Result<Vec<Row>, CopError> {
    for pair in reader.scan(ranges, start_ts, descending)? {
        processor.process(&pair)?;
    }
    processor.finish()
}

/// 自底向上收集执行器链表（子节点在前）。
pub fn get_executor_list(root: &Executor) -> Vec<&Executor> {
    let mut result = Vec::new();
    fn visit<'a>(executor: &'a Executor, output: &mut Vec<&'a Executor>) {
        let child = match executor {
            Executor::Selection { child, .. }
            | Executor::Limit { child, .. }
            | Executor::TopN { child, .. }
            | Executor::Projection { child, .. }
            | Executor::Expand { child, .. }
            | Executor::Aggregation { child, .. }
            | Executor::ExchangeSender { child, .. } => Some(child.as_ref()),
            _ => None,
        };
        if let Some(child) = child {
            visit(child, output);
        }
        output.push(executor);
    }
    visit(root, &mut result);
    result
}

/// 在执行器列表中查找 TableScan/IndexScan。
pub fn get_scan_executor(root: &Executor) -> Result<&Executor, CopError> {
    get_executor_list(root)
        .into_iter()
        .find(|executor| {
            matches!(
                executor,
                Executor::TableScan { .. } | Executor::IndexScan { .. }
            )
        })
        .ok_or(CopError::InvalidRequest("scan executor not found".into()))
}

/// 将行列表按 ROWS_PER_CHUNK 语义追加进 Chunk 向量。
pub fn chunks_from_rows(rows: Vec<Row>) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    for row in rows {
        append_row(&mut chunks, row);
    }
    chunks
}

/// 字节切片深拷贝（对齐 Go safeCopy）。
pub fn safe_copy(value: &[u8]) -> Vec<u8> {
    value.to_vec()
}
/// 判断 start_ts 是否已在 resolved 列表中。
pub fn is_resolved(start_ts: u64, resolved: &[u64]) -> bool {
    resolved.contains(&start_ts)
}
/// 若 end 非空且 current >= end，则已越过扫描上界。
pub fn exceed_end_key(current: &[u8], end: &[u8]) -> bool {
    !end.is_empty() && current >= end
}

/// 计算 key 的下一前缀（末位非 0xff 加一并将进位后缀清零；全 0xff 则追加 0）。
fn prefix_next(key: &[u8]) -> Vec<u8> {
    let mut result = key.to_vec();
    for index in (0..result.len()).rev() {
        if result[index] != u8::MAX {
            result[index] += 1;
            result[index + 1..].fill(0);
            return result;
        }
    }
    result.push(0);
    result
}
