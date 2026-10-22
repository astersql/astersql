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

// Index LookUp Merge Join（索引查找归并连接）执行器。
//
// 外表按 join key 有序（或先排序）批量 lookup 内表；inner worker 取回内表行后
// 按排序 key 做 merge join（归并连接）：双指针推进相同 key 组并产出结果。
// 适用于内外表均按连接键有序的场景。

use crate::index_lookup_join::{
    IndexJoinExecutorBuilder, IndexJoinLookupContent, compare_row, extract_key,
};
use crate::joiner::{Joiner, NaajType, Row};
use crate::row_table_builder::Value;
use astersql_util_execdetails::execdetails::RuntimeStatsColl;
use std::sync::{Arc, Mutex};

/// 一批外表行的 merge-join 任务：含 lookup 内容、内表行与输出。
#[derive(Clone, Debug, Default)]
pub struct LookUpMergeJoinTask {
    /// 本批外表行（按 join key 有序）。
    pub outer_rows: Vec<Row>,
    /// 各外表行是否参与 join（保留 Go outerMatch）。
    pub outer_match: Vec<bool>,
    /// 去重后的外表 lookup key 列表。
    pub lookup_contents: Vec<IndexJoinLookupContent>,
    /// 按 key 有序的内表行。
    pub inner_rows: Vec<Row>,
    /// 本任务产出的连接结果。
    pub output: Vec<Row>,
    /// inner worker 是否已完成。
    pub done: bool,
}
/// `next` 返回的结果块：行集与可选错误。
#[derive(Clone, Debug, Default)]
pub struct IndexMergeJoinResult {
    /// 结果行。
    pub rows: Vec<Row>,
    /// 执行错误（若有）。
    pub error: Option<String>,
}

/// 外表 merge worker：可选排序后按相同 key 组切批构造任务。
pub struct OuterMergeWorker {
    /// 外表行（可能已按 key 排序）。
    rows: Vec<Row>,
    /// 外表 join key 列。
    key_columns: Vec<usize>,
    /// 读取游标。
    cursor: usize,
    /// 当前目标批大小。
    batch_size: usize,
    /// 初始批大小，重开执行器时恢复。
    initial_batch_size: usize,
    /// 批大小上限。
    max_batch_size: usize,
}
impl OuterMergeWorker {
    /// 默认需要外表排序的构造入口。
    pub fn new(
        rows: Vec<Row>,
        key_columns: Vec<usize>,
        batch_size: usize,
        max_batch_size: usize,
    ) -> Result<Self, String> {
        Self::new_with_outer_sort(rows, key_columns, batch_size, max_batch_size, true)
    }

    /// 可选先按 join key 排序外表，再构造 worker。
    pub fn new_with_outer_sort(
        mut rows: Vec<Row>,
        key_columns: Vec<usize>,
        batch_size: usize,
        max_batch_size: usize,
        need_outer_sort: bool,
    ) -> Result<Self, String> {
        if batch_size == 0 || max_batch_size == 0 {
            return Err("merge lookup batch size must be positive".into());
        }
        if need_outer_sort {
            rows.sort_by(|left, right| {
                compare_keys(left, right, &key_columns, &key_columns)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        Ok(Self {
            rows,
            key_columns,
            cursor: 0,
            batch_size,
            initial_batch_size: batch_size,
            max_batch_size,
        })
    }
    /// 切下一批外表行；若批尾与下一批 key 相同则扩展，避免拆散同 key 组。
    pub fn build_task(&mut self) -> Option<LookUpMergeJoinTask> {
        if self.cursor >= self.rows.len() {
            return None;
        }
        let mut end = (self.cursor + self.batch_size).min(self.rows.len());
        // 同 key 组必须完整落入同一批，否则 merge join 会漏匹配。
        while end < self.rows.len()
            && compare_keys(
                &self.rows[end - 1],
                &self.rows[end],
                &self.key_columns,
                &self.key_columns,
            )
            .is_ok_and(|order| order.is_eq())
        {
            end += 1;
        }
        let rows = self.rows[self.cursor..end].to_vec();
        self.cursor = end;
        self.batch_size = (self.batch_size * 2).min(self.max_batch_size);
        Some(LookUpMergeJoinTask {
            outer_match: vec![true; rows.len()],
            outer_rows: rows,
            ..LookUpMergeJoinTask::default()
        })
    }

    /// 复位外表读取状态，供执行器 `Close` 后再次 `Open`。
    fn reset(&mut self) {
        self.cursor = 0;
        self.batch_size = self.initial_batch_size;
    }
}

/// 内表 merge worker：lookup 取回内表后按 key 做双指针归并连接。
pub struct InnerMergeWorker<'a> {
    /// 按 lookup key 拉取内表的 builder。
    builder: &'a dyn IndexJoinExecutorBuilder,
    /// 外表 join key 列。
    outer_key_columns: &'a [usize],
    /// 内表 join key 列。
    inner_key_columns: &'a [usize],
    /// 连接器。
    joiner: &'a Joiner,
}
impl InnerMergeWorker<'_> {
    /// 从外表行构造非 NULL lookup key 并去重。
    pub fn construct_lookup_keys(&self, task: &mut LookUpMergeJoinTask) -> Result<(), String> {
        for row in &task.outer_rows {
            let keys = extract_key(row, self.outer_key_columns)?;
            if keys.iter().any(|value| matches!(value, Value::Null)) {
                continue;
            }
            task.lookup_contents.push(IndexJoinLookupContent {
                keys,
                row: row.clone(),
                key_columns: self.inner_key_columns.to_vec(),
                key_column_ids: Vec::new(),
            });
        }
        task.lookup_contents
            .dedup_by(|left, right| left.keys == right.keys);
        Ok(())
    }
    /// 拉取内表、按 key 排序后执行 merge join。
    pub fn handle_task(&self, task: &mut LookUpMergeJoinTask) -> Result<(), String> {
        self.construct_lookup_keys(task)?;
        task.inner_rows = self.builder.build(&task.lookup_contents)?;
        // 内表必须有序，merge join 才能用双指针推进。
        task.inner_rows.sort_by(|left, right| {
            compare_keys(left, right, self.inner_key_columns, self.inner_key_columns)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        self.do_merge_join(task)?;
        task.done = true;
        Ok(())
    }
    /// 双指针归并：相等则扩展同 key 组并 Joiner 匹配，外表更小则 miss，内表更小则跳过。
    fn do_merge_join(&self, task: &mut LookUpMergeJoinTask) -> Result<(), String> {
        let mut outer = 0;
        let mut inner = 0;
        while outer < task.outer_rows.len() {
            if inner >= task.inner_rows.len() {
                self.joiner
                    .on_miss_match(false, &task.outer_rows[outer], &mut task.output);
                outer += 1;
                continue;
            }
            match compare_keys(
                &task.outer_rows[outer],
                &task.inner_rows[inner],
                self.outer_key_columns,
                self.inner_key_columns,
            )? {
                std::cmp::Ordering::Less => {
                    self.joiner
                        .on_miss_match(false, &task.outer_rows[outer], &mut task.output);
                    outer += 1;
                }
                std::cmp::Ordering::Greater => inner += 1,
                std::cmp::Ordering::Equal => {
                    let outer_start = outer;
                    let inner_start = inner;
                    // 扩展外表同 key 区间。
                    while outer < task.outer_rows.len()
                        && compare_keys(
                            &task.outer_rows[outer_start],
                            &task.outer_rows[outer],
                            self.outer_key_columns,
                            self.outer_key_columns,
                        )?
                        .is_eq()
                    {
                        outer += 1;
                    }
                    // 扩展内表同 key 区间。
                    while inner < task.inner_rows.len()
                        && compare_keys(
                            &task.inner_rows[inner_start],
                            &task.inner_rows[inner],
                            self.inner_key_columns,
                            self.inner_key_columns,
                        )?
                        .is_eq()
                    {
                        inner += 1;
                    }
                    for outer_row in &task.outer_rows[outer_start..outer] {
                        let result = self.joiner.try_to_match_inners(
                            outer_row,
                            &task.inner_rows[inner_start..inner],
                            &mut task.output,
                            NaajType::Unknown,
                        )?;
                        if !result.matched {
                            self.joiner
                                .on_miss_match(result.has_null, outer_row, &mut task.output);
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// Index LookUp Merge Join 执行器：外表有序分批 lookup，内表归并连接。
pub struct IndexLookUpMergeJoin {
    /// 外表分批与可选排序 worker。
    outer_worker: OuterMergeWorker,
    /// 内表索引 lookup builder。
    builder: Box<dyn IndexJoinExecutorBuilder>,
    /// 外表 join key 列。
    outer_key_columns: Vec<usize>,
    /// 内表 join key 列。
    inner_key_columns: Vec<usize>,
    /// 连接器。
    joiner: Joiner,
    /// 已产出结果缓冲。
    output: Vec<Row>,
    /// 结果读取游标。
    cursor: usize,
    /// 是否已 open。
    opened: bool,
    executed: bool,
}
impl IndexLookUpMergeJoin {
    /// Bind worker candidate evidence to this statement and plan ID.
    pub fn with_runtime_stats(
        mut self,
        plan_id: i32,
        collection: Arc<Mutex<RuntimeStatsColl>>,
    ) -> Self {
        self.joiner = self.joiner.with_runtime_stats(plan_id, collection);
        self
    }

    /// 构造执行器（默认对外表按 key 排序）。
    pub fn new(
        outer_rows: Vec<Row>,
        outer_key_columns: Vec<usize>,
        inner_key_columns: Vec<usize>,
        builder: Box<dyn IndexJoinExecutorBuilder>,
        joiner: Joiner,
        batch_size: usize,
        max_batch_size: usize,
    ) -> Result<Self, String> {
        Self::new_with_outer_sort(
            outer_rows,
            outer_key_columns,
            inner_key_columns,
            builder,
            joiner,
            batch_size,
            max_batch_size,
            true,
        )
    }

    /// 可选跳过外表排序（调用方保证已有序）的构造入口。
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_outer_sort(
        outer_rows: Vec<Row>,
        outer_key_columns: Vec<usize>,
        inner_key_columns: Vec<usize>,
        builder: Box<dyn IndexJoinExecutorBuilder>,
        joiner: Joiner,
        batch_size: usize,
        max_batch_size: usize,
        need_outer_sort: bool,
    ) -> Result<Self, String> {
        if outer_key_columns.len() != inner_key_columns.len() {
            return Err("index merge join key count mismatch".into());
        }
        Ok(Self {
            outer_worker: OuterMergeWorker::new_with_outer_sort(
                outer_rows,
                outer_key_columns.clone(),
                batch_size,
                max_batch_size,
                need_outer_sort,
            )?,
            builder,
            outer_key_columns,
            inner_key_columns,
            joiner,
            output: Vec::new(),
            cursor: 0,
            opened: false,
            executed: false,
        })
    }
    /// 打开执行器并清空输出缓冲。
    pub fn open(&mut self) -> Result<(), String> {
        self.joiner.reset_joined_rows();
        self.executed = false;
        self.output.clear();
        self.cursor = 0;
        self.outer_worker.reset();
        self.opened = true;
        Ok(())
    }
    /// 消费全部外表任务，将各任务 merge 结果追加到 `output`。
    fn execute(&mut self) -> Result<(), String> {
        while let Some(mut task) = self.outer_worker.build_task() {
            InnerMergeWorker {
                builder: self.builder.as_ref(),
                outer_key_columns: &self.outer_key_columns,
                inner_key_columns: &self.inner_key_columns,
                joiner: &self.joiner,
            }
            .handle_task(&mut task)?;
            self.output.extend(task.output);
        }
        Ok(())
    }
    /// 惰性执行并按 `required_rows` 切片返回结果。
    pub fn next(&mut self, required_rows: usize) -> Result<IndexMergeJoinResult, String> {
        if !self.opened {
            self.open()?;
        }
        if !self.executed {
            self.execute()?;
            self.executed = true;
        }
        if self.cursor >= self.output.len() || required_rows == 0 {
            return Ok(IndexMergeJoinResult::default());
        }
        let end = (self.cursor + required_rows).min(self.output.len());
        let rows = self.output[self.cursor..end].to_vec();
        self.cursor = end;
        Ok(IndexMergeJoinResult { rows, error: None })
    }
    /// 关闭执行器。
    pub fn close(&mut self) {
        self.joiner.register_joined_rows();
        self.output.clear();
        self.cursor = 0;
        self.outer_worker.reset();
        self.opened = false;
    }
}

/// 比较外表行与内表行在各自 key 列上的大小关系。
pub fn compare_keys(
    outer: &Row,
    inner: &Row,
    outer_columns: &[usize],
    inner_columns: &[usize],
) -> Result<std::cmp::Ordering, String> {
    if outer_columns.len() != inner_columns.len() {
        return Err("merge key count mismatch".into());
    }
    Ok(compare_row(
        &extract_key(outer, outer_columns)?,
        &extract_key(inner, inner_columns)?,
    ))
}
