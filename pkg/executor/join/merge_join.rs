// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Merge Join（排序归并连接，Sort-Merge Join / SMJ）执行器。
//
// 要求两侧输入已按 join key 有序。算法按等值分组（group）同步推进 outer/inner：
// key 更小的一侧输出未匹配行（outer join）或跳过（inner），相等则交叉匹配。
// `ShuffleMergeJoinExec` 先按 key 哈希分到多个 lane，再并行执行归并连接。

use crate::index_lookup_join::{compare_row, extract_key};
use crate::joiner::{Joiner, NaajType, Row};
use crate::row_table_builder::Value;
use astersql_util_execdetails::execdetails::RuntimeStatsColl;
use std::sync::{Arc, Mutex};

/// Merge Join 一侧输入表：已按 join key 有序的行集，支持按等值分组迭代。
#[derive(Clone, Debug, Default)]
pub struct MergeJoinTable {
    /// 有序输入行。
    pub rows: Vec<Row>,
    /// Join key 列下标。
    pub join_keys: Vec<usize>,
    /// 是否为 inner 侧（inner 侧 NULL key 组会被跳过）。
    pub is_inner: bool,
    cursor: usize,
    group_start: usize,
    group_end: usize,
    finished: bool,
}

impl MergeJoinTable {
    /// 构造表并校验 join key 列可抽取。
    pub fn new(rows: Vec<Row>, join_keys: Vec<usize>, is_inner: bool) -> Result<Self, String> {
        for row in &rows {
            extract_key(row, &join_keys)?;
        }
        Ok(Self {
            rows,
            join_keys,
            is_inner,
            cursor: 0,
            group_start: 0,
            group_end: 0,
            finished: false,
        })
    }
    /// 重置分组游标，准备从头读取。
    pub fn init(&mut self) {
        self.cursor = 0;
        self.group_start = 0;
        self.group_end = 0;
        self.finished = self.rows.is_empty();
    }
    /// 标记输入已耗尽并收拢游标。
    pub fn finish(&mut self) {
        self.finished = true;
        self.cursor = self.rows.len();
        self.group_start = self.cursor;
        self.group_end = self.cursor;
    }
    /// 取出下一组 join key 相等的连续行；耗尽时返回 `None`。
    pub fn select_next_group(&mut self) -> Result<Option<&[Row]>, String> {
        loop {
            if self.cursor >= self.rows.len() {
                self.finish();
                return Ok(None);
            }
            self.group_start = self.cursor;
            self.cursor += 1;
            // 扩展当前 group，直到 key 变化或输入结束。
            while self.cursor < self.rows.len()
                && self.compare_rows(self.group_start, self.cursor)?.is_eq()
            {
                self.cursor += 1;
            }
            self.group_end = self.cursor;
            // Go 的 inner 分组会丢弃任一 join key 为 NULL 的整组。
            if !self.is_inner
                || !Self::has_null_in_join_key(&self.rows[self.group_start], &self.join_keys)
            {
                return Ok(Some(&self.rows[self.group_start..self.group_end]));
            }
        }
    }
    /// 返回当前 group 的行切片。
    pub fn current_group(&self) -> &[Row] {
        &self.rows[self.group_start..self.group_end]
    }
    /// 返回当前 group 代表行的 join key；无组时为 `None`。
    pub fn current_key(&self) -> Result<Option<Row>, String> {
        if self.group_start >= self.rows.len() {
            return Ok(None);
        }
        Ok(Some(extract_key(
            &self.rows[self.group_start],
            &self.join_keys,
        )?))
    }
    /// 判断行的任一 join key 是否为 NULL（NULL 不参与等值匹配）。
    pub fn has_null_in_join_key(row: &Row, join_keys: &[usize]) -> bool {
        join_keys.iter().any(|index| {
            row.get(*index)
                .is_none_or(|value| matches!(value, Value::Null))
        })
    }
    fn compare_rows(&self, left: usize, right: usize) -> Result<std::cmp::Ordering, String> {
        Ok(compare_row(
            &extract_key(&self.rows[left], &self.join_keys)?,
            &extract_key(&self.rows[right], &self.join_keys)?,
        ))
    }
}

/// Merge Join 执行器：同步推进 outer/inner 有序分组并产出连接结果。
pub struct MergeJoinExec {
    /// Outer 侧输入表。
    pub outer_table: MergeJoinTable,
    /// Inner 侧输入表。
    pub inner_table: MergeJoinTable,
    /// 按 Join 类型拼装匹配/未匹配结果的 Joiner。
    pub joiner: Joiner,
    output: Vec<Row>,
    cursor: usize,
    opened: bool,
    executed: bool,
    closed: bool,
}

impl MergeJoinExec {
    /// Bind worker candidate evidence to this statement and plan ID.
    pub fn with_runtime_stats(
        mut self,
        plan_id: i32,
        collection: Arc<Mutex<RuntimeStatsColl>>,
    ) -> Self {
        self.joiner = self.joiner.with_runtime_stats(plan_id, collection);
        self
    }

    /// 构造执行器；两侧 join key 列数必须一致。
    pub fn new(
        outer_table: MergeJoinTable,
        inner_table: MergeJoinTable,
        joiner: Joiner,
    ) -> Result<Self, String> {
        if outer_table.join_keys.len() != inner_table.join_keys.len() {
            return Err("merge join key count mismatch".into());
        }
        Ok(Self {
            outer_table,
            inner_table,
            joiner,
            output: Vec::new(),
            cursor: 0,
            opened: false,
            executed: false,
            closed: false,
        })
    }
    /// 打开执行器并初始化两侧表游标。
    pub fn open(&mut self) -> Result<(), String> {
        self.joiner.reset_joined_rows();
        self.executed = false;
        self.closed = false;
        self.outer_table.init();
        self.inner_table.init();
        self.output.clear();
        self.cursor = 0;
        self.opened = true;
        Ok(())
    }
    /// 一次性归并两侧全部分组，结果写入 `output`。
    fn execute(&mut self) -> Result<(), String> {
        let mut outer_group = self.outer_table.select_next_group()?.map(<[Row]>::to_vec);
        let mut inner_group = self.inner_table.select_next_group()?.map(<[Row]>::to_vec);
        while outer_group.is_some() {
            let outers = outer_group.as_ref().unwrap().clone();
            // Outer key 含 NULL 时无法匹配，按 miss 输出。
            let outer_null = outers.first().is_some_and(|row| {
                MergeJoinTable::has_null_in_join_key(row, &self.outer_table.join_keys)
            });
            if outer_null {
                for outer in &outers {
                    self.joiner.on_miss_match(false, outer, &mut self.output);
                }
                outer_group = self.outer_table.select_next_group()?.map(<[Row]>::to_vec);
                continue;
            }
            let Some(inners) = inner_group.as_ref() else {
                // Inner 已耗尽：剩余 outer 一律 miss。
                for outer in &outers {
                    self.joiner.on_miss_match(false, outer, &mut self.output);
                }
                outer_group = self.outer_table.select_next_group()?.map(<[Row]>::to_vec);
                continue;
            };
            let comparison = self.compare(&outers[0], &inners[0])?;
            match comparison {
                std::cmp::Ordering::Less => {
                    // Outer key 更小：输出未匹配 outer，推进 outer。
                    for outer in &outers {
                        self.joiner.on_miss_match(false, outer, &mut self.output);
                    }
                    outer_group = self.outer_table.select_next_group()?.map(<[Row]>::to_vec);
                }
                std::cmp::Ordering::Greater => {
                    // Inner key 更小：跳过该 inner 组。
                    inner_group = self.inner_table.select_next_group()?.map(<[Row]>::to_vec)
                }
                std::cmp::Ordering::Equal => {
                    // Key 相等：对每个 outer 行尝试匹配整组 inner。
                    for outer in &outers {
                        let result = self.joiner.try_to_match_inners(
                            outer,
                            inners,
                            &mut self.output,
                            NaajType::Unknown,
                        )?;
                        if !result.matched {
                            self.joiner
                                .on_miss_match(result.has_null, outer, &mut self.output);
                        }
                    }
                    outer_group = self.outer_table.select_next_group()?.map(<[Row]>::to_vec);
                    inner_group = self.inner_table.select_next_group()?.map(<[Row]>::to_vec);
                }
            }
        }
        Ok(())
    }
    /// 比较 outer/inner 代表行的 join key 顺序。
    pub fn compare(&self, outer: &Row, inner: &Row) -> Result<std::cmp::Ordering, String> {
        Ok(compare_row(
            &extract_key(outer, &self.outer_table.join_keys)?,
            &extract_key(inner, &self.inner_table.join_keys)?,
        ))
    }
    /// 拉取最多 `required_rows` 行结果；首次调用时触发完整归并。
    pub fn next(&mut self, required_rows: usize) -> Result<Vec<Row>, String> {
        if self.closed {
            return Err("join is closed; call open before reuse".into());
        }
        if !self.opened {
            self.open()?;
        }
        if !self.executed {
            self.execute()?;
            self.executed = true;
        }
        if self.cursor >= self.output.len() || required_rows == 0 {
            return Ok(Vec::new());
        }
        let end = (self.cursor + required_rows).min(self.output.len());
        let rows = self.output[self.cursor..end].to_vec();
        self.cursor = end;
        Ok(rows)
    }
    /// 关闭两侧输入并清空缓冲。
    pub fn close(&mut self) {
        self.joiner.register_joined_rows();
        self.outer_table.finish();
        self.inner_table.finish();
        self.output.clear();
        self.opened = false;
        self.closed = true;
    }
}

/// 先按 join key 哈希分 lane，再并行执行多个 Merge Join 的 Shuffle 变体。
pub struct ShuffleMergeJoinExec {
    lanes: Vec<MergeJoinExec>,
    output: Vec<Row>,
    cursor: usize,
    opened: bool,
    closed: bool,
}

impl ShuffleMergeJoinExec {
    /// 按 concurrency 将 outer/inner 行分区到各 lane 并构造子执行器。
    pub fn new(
        outer_rows: Vec<Row>,
        inner_rows: Vec<Row>,
        outer_join_keys: Vec<usize>,
        inner_join_keys: Vec<usize>,
        joiner: Joiner,
        concurrency: usize,
    ) -> Result<Self, String> {
        if concurrency == 0 {
            return Err("shuffle merge join concurrency must be positive".into());
        }
        if outer_join_keys.len() != inner_join_keys.len() {
            return Err("shuffle merge join key count mismatch".into());
        }
        let mut outer_lanes = vec![Vec::new(); concurrency];
        let mut inner_lanes = vec![Vec::new(); concurrency];
        // 同一 key 的 outer/inner 必须落入相同 lane，保证局部归并可正确匹配。
        for row in outer_rows {
            let lane = Self::partition(&row, &outer_join_keys, concurrency)?;
            outer_lanes[lane].push(row);
        }
        for row in inner_rows {
            let lane = Self::partition(&row, &inner_join_keys, concurrency)?;
            inner_lanes[lane].push(row);
        }
        let lanes = outer_lanes
            .into_iter()
            .zip(inner_lanes)
            .map(|(outer, inner)| {
                MergeJoinExec::new(
                    MergeJoinTable::new(outer, outer_join_keys.clone(), false)?,
                    MergeJoinTable::new(inner, inner_join_keys.clone(), true)?,
                    joiner.clone(),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            lanes,
            output: Vec::new(),
            cursor: 0,
            opened: false,
            closed: false,
        })
    }

    /// 按 join key 哈希取模，决定行所属 lane。
    fn partition(row: &Row, keys: &[usize], concurrency: usize) -> Result<usize, String> {
        use std::hash::{Hash, Hasher};

        let key = extract_key(row, keys)?;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        format!("{key:?}").hash(&mut hasher);
        Ok(hasher.finish() as usize % concurrency)
    }

    /// 打开 Shuffle Merge Join；关闭后不可再次打开。
    pub fn open(&mut self) -> Result<(), String> {
        if self.closed {
            return Err("cannot reopen closed shuffle merge join".into());
        }
        self.output.clear();
        self.cursor = 0;
        self.opened = true;
        Ok(())
    }

    /// 在线程作用域内并行跑完各 lane，合并输出。
    fn execute(&mut self) -> Result<(), String> {
        let lanes = std::mem::take(&mut self.lanes);
        let lane_results = std::thread::scope(|scope| {
            lanes
                .into_iter()
                .map(|mut lane| {
                    scope.spawn(move || {
                        lane.open()?;
                        let mut output = Vec::new();
                        loop {
                            let rows = lane.next(1024)?;
                            if rows.is_empty() {
                                break;
                            }
                            output.extend(rows);
                        }
                        lane.close();
                        Ok::<Vec<Row>, String>(output)
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .map_err(|_| "shuffle merge join worker panicked".to_string())?
                })
                .collect::<Result<Vec<_>, String>>()
        })?;
        self.output = lane_results.into_iter().flatten().collect();
        Ok(())
    }

    /// 拉取结果行；首次调用时触发各 lane 并行执行。
    pub fn next(&mut self, required_rows: usize) -> Result<Vec<Row>, String> {
        if !self.opened {
            self.open()?;
        }
        if required_rows == 0 {
            return Ok(Vec::new());
        }
        if self.output.is_empty() && self.cursor == 0 {
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

    /// 关闭所有 lane 并清空状态。
    pub fn close(&mut self) {
        for lane in &mut self.lanes {
            lane.close();
        }
        self.lanes.clear();
        self.output.clear();
        self.opened = false;
        self.closed = true;
    }
}
