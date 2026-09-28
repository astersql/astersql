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

// Merge Join 执行器如何按有序分组推进两侧输入。
// MergeJoinExec implements the merge join algorithm.
// This operator assumes that two iterators of both sides
// will provide required order on join condition:
// 1. For equal-join, one of the join key from each side
// matches the order given.
// 2. For other cases its preferred not to use SMJ and operator
// will throw error.
// pub struct MergeJoinExec {
//     pub BaseExecutor: exec::BaseExecutor,
//     pub StmtCtx: *mut stmtctx::StatementContext,
//     pub CompareFuncs: Vec<expression::CompareFunc>,
//     pub Joiner: Box<dyn Joiner>,
//     pub IsOuterJoin: bool,
//     pub Desc: bool,
//     pub InnerTable: MergeJoinTable,
//     pub OuterTable: MergeJoinTable,
//     pub hasMatch: bool,
//     pub hasNull: bool,
//     pub memTracker: Option<memory::Tracker>,
//     pub diskTracker: Option<disk::Tracker>,
// }
// MergeJoinTable is used for merge join
// pub struct MergeJoinTable {
//     pub inited: bool,
//     pub IsInner: bool,
//     pub ChildIndex: usize,
//     pub JoinKeys: Vec<expression::Column>,
//     pub Filters: Vec<expression::Expression>,
//     pub executed: bool,
//     pub childChunk: Option<chunk::Chunk>,
//     pub childChunkIter: Option<chunk::Iterator4Chunk>,
//     pub groupChecker: Option<vecgroupchecker::VecGroupChecker>,
//     pub groupRowsSelected: Vec<usize>,
//     pub groupRowsIter: chunk::Iterator,
// for inner table, an unbroken group may refer many chunks
//     pub rowContainer: Option<chunk::RowContainer>,
// for outer table, save result of filters
//     pub filtersSelected: Vec<bool>,
//     pub memTracker: Option<memory::Tracker>,
// }
// impl MergeJoinTable {
// init 对应 Go 方法：为一侧输入准备 chunk、分组检查器和内存/磁盘追踪器。
//     pub fn init(&mut self, executor: &mut MergeJoinExec) {
//         let child = executor.Children(self.ChildIndex);
//         self.childChunk = Some(exec::TryNewCacheChunk(child));
//         self.childChunkIter = Some(chunk::NewIterator4Chunk(self.childChunk.as_mut().unwrap()));
//         let mut items = Vec::with_capacity(self.JoinKeys.len());
//         for col in &self.JoinKeys {
//             items.push(col.clone());
//         }
//         let vecEnabled = executor.Ctx().GetSessionVars().EnableVectorizedExpression;
//         self.groupChecker = Some(vecgroupchecker::NewVecGroupChecker(
//             executor.Ctx().GetExprCtx().GetEvalCtx(),
//             vecEnabled,
//             items,
//         ));
//         self.groupRowsIter = chunk::NewIterator4Chunk(self.childChunk.as_mut().unwrap());
//         if self.IsInner {
//             let mut container = chunk::NewRowContainer(child.RetFieldTypes(), self.childChunk.as_ref().unwrap().Capacity());
//             container.GetMemTracker().AttachTo(executor.memTracker.as_mut().unwrap());
//             container.GetMemTracker().SetLabel(memory::LabelForInnerTable);
//             container.GetDiskTracker().AttachTo(executor.diskTracker.as_mut().unwrap());
//             container.GetDiskTracker().SetLabel(memory::LabelForInnerTable);
//             if vardef::EnableTmpStorageOnOOM.Load() {
// Go 这里通过 failpoint 替换 spill action；保留测试注入点的控制语义。
//                 let mut actionSpill = container.ActionSpill();
//                 failpoint::Inject("testMergeJoinRowContainerSpill", |val| {
//                     if val.as_bool() {
//                         actionSpill = container.ActionSpillForTest();
//                     }
//                 });
//                 executor.Ctx().GetSessionVars().MemTracker.FallbackOldAndSetNewAction(actionSpill);
//             }
//             self.rowContainer = Some(container);
//             self.memTracker = Some(memory::NewTracker(memory::LabelForInnerTable, -1));
//         } else {
//             self.filtersSelected = Vec::with_capacity(executor.MaxChunkSize());
//             self.memTracker = Some(memory::NewTracker(memory::LabelForOuterTable, -1));
//         }
//         self.memTracker.as_mut().unwrap().AttachTo(executor.memTracker.as_mut().unwrap());
//         self.inited = true;
//         self.memTracker
//             .as_mut()
//             .unwrap()
//             .Consume(self.childChunk.as_ref().unwrap().MemoryUsage());
//     }
// finish 对应 Go 资源收尾：归还 chunk 内存，关闭 inner rowContainer，并清空临时状态。
//     pub fn finish(&mut self) -> Result<(), errors::Error> {
//         if !self.inited {
//             return Ok(());
//         }
//         if let (Some(mem), Some(chunk)) = (self.memTracker.as_mut(), self.childChunk.as_ref()) {
//             mem.Consume(-chunk.MemoryUsage());
//         }
//         if self.IsInner {
//             failpoint::Inject("testMergeJoinRowContainerSpill", |val| {
//                 if val.as_bool() {
//                     let actionSpill = self.rowContainer.as_ref().unwrap().ActionSpill();
//                     actionSpill.WaitForTest();
//                 }
//             });
//             if let Some(container) = self.rowContainer.as_mut() {
//                 container.Close()?;
//             }
//         }
//         self.executed = false;
//         self.childChunk = None;
//         self.childChunkIter = None;
//         self.groupChecker = None;
//         self.groupRowsSelected.clear();
//         self.groupRowsIter = chunk::Iterator::default();
//         self.rowContainer = None;
//         self.filtersSelected.clear();
//         self.memTracker = None;
//         Ok(())
//     }
// selectNextGroup 对应 Go 方法：从 groupChecker 取下一组，inner 侧跳过 join key 为 NULL 的组。
//     pub fn selectNextGroup(&mut self) {
//         self.groupRowsSelected.clear();
//         let (begin, end) = self.groupChecker.as_mut().unwrap().GetNextGroup();
//         if self.IsInner && self.hasNullInJoinKey(self.childChunk.as_ref().unwrap().GetRow(begin)) {
//             return;
//         }
//         for i in begin..end {
//             self.groupRowsSelected.push(i);
//         }
//         self.childChunk.as_mut().unwrap().SetSel(self.groupRowsSelected.clone());
//     }
// fetchNextChunk 对应 Go 的 exec.Next 包装，额外维护 child chunk 内存变化。
//     pub fn fetchNextChunk(&mut self, ctx: context::Context, executor: &mut MergeJoinExec) -> Result<(), errors::Error> {
//         let oldMemUsage = self.childChunk.as_ref().unwrap().MemoryUsage();
//         exec::Next(ctx, executor.Children(self.ChildIndex), self.childChunk.as_mut().unwrap())?;
//         let delta = self.childChunk.as_ref().unwrap().MemoryUsage() - oldMemUsage;
//         self.memTracker.as_mut().unwrap().Consume(delta);
//         self.executed = self.childChunk.as_ref().unwrap().NumRows() == 0;
//         Ok(())
//     }
// fetchNextInnerGroup 对应 Go 的 inner 侧分组读取。
// 它会把跨 chunk 的同 key 行放入 rowContainer，直到得到一个完整非空 inner group。
//     pub fn fetchNextInnerGroup(&mut self, ctx: context::Context, exec_: &mut MergeJoinExec) -> Result<(), errors::Error> {
//         self.childChunk.as_mut().unwrap().SetSel(None);
//         self.rowContainer.as_mut().unwrap().Reset()?;
//         loop {
//             if self.executed && self.groupChecker.as_ref().unwrap().IsExhausted() {
// Go 在清除 sel 后强制 iterator 到末尾，避免后续误读旧行。
//                 self.groupRowsIter.ReachEnd();
//                 return Ok(());
//             }
//             let mut isEmpty = true;
//             while isEmpty && !self.groupChecker.as_ref().unwrap().IsExhausted() {
//                 self.selectNextGroup();
//                 isEmpty = self.groupRowsSelected.is_empty();
//             }
//             while !self.executed && self.groupChecker.as_ref().unwrap().IsExhausted() {
//                 if !isEmpty {
// 当前 group 还可能跨 chunk，把 childChunk 所有权交给 RowContainer。
//                     self.rowContainer.as_mut().unwrap().Add(self.childChunk.as_mut().unwrap())?;
//                     self.memTracker
//                         .as_mut()
//                         .unwrap()
//                         .Consume(-self.childChunk.as_ref().unwrap().MemoryUsage());
//                     self.groupRowsSelected.clear();
//                     self.childChunk = Some(self.rowContainer.as_mut().unwrap().AllocChunk());
//                     self.childChunkIter = Some(chunk::NewIterator4Chunk(self.childChunk.as_mut().unwrap()));
//                     self.memTracker
//                         .as_mut()
//                         .unwrap()
//                         .Consume(self.childChunk.as_ref().unwrap().MemoryUsage());
//                 }
//                 self.fetchNextChunk(ctx.clone(), exec_)?;
//                 if self.executed {
//                     break;
//                 }
//                 let isFirstGroupSameAsPrev = self
//                     .groupChecker
//                     .as_mut()
//                     .unwrap()
//                     .SplitIntoGroups(self.childChunk.as_ref().unwrap())?;
//                 if isFirstGroupSameAsPrev && !isEmpty {
//                     self.selectNextGroup();
//                 }
//             }
//             if !isEmpty {
//                 break;
//             }
//         }
//         let mut iter = chunk::Iterator::default();
//         if self.rowContainer.as_ref().unwrap().NumChunks() != 0 {
//             iter = chunk::NewIterator4RowContainer(self.rowContainer.as_ref().unwrap());
//         }
//         if !self.groupRowsSelected.is_empty() {
//             iter = if !iter.is_nil() {
//                 chunk::NewMultiIterator(iter, self.childChunkIter.as_ref().unwrap().clone())
//             } else {
//                 self.childChunkIter.as_ref().unwrap().clone().into()
//             };
//         }
//         self.groupRowsIter = iter;
//         self.groupRowsIter.Begin();
//         Ok(())
//     }
// fetchNextOuterGroup 对应 Go 的 outer 侧分组读取：outer group 只保证在当前 chunk 内完整。
//     pub fn fetchNextOuterGroup(
//         &mut self,
//         ctx: context::Context,
//         exec_: &mut MergeJoinExec,
//         requiredRows: usize,
//     ) -> Result<(), errors::Error> {
//         if self.executed && self.groupChecker.as_ref().unwrap().IsExhausted() {
//             return Ok(());
//         }
//         if !self.executed && self.groupChecker.as_ref().unwrap().IsExhausted() {
// 没有 filter 且是 outer join 时，把下游需要的行数传给 child，减少无用读取。
//             if exec_.IsOuterJoin && self.Filters.is_empty() {
//                 self.childChunk.as_mut().unwrap().SetRequiredRows(requiredRows, exec_.MaxChunkSize());
//             }
//             self.fetchNextChunk(ctx, exec_)?;
//             if self.executed {
//                 return Ok(());
//             }
//             self.childChunkIter.as_mut().unwrap().Begin();
//             self.filtersSelected = expression::VectorizedFilter(
//                 exec_.Ctx().GetExprCtx().GetEvalCtx(),
//                 exec_.Ctx().GetSessionVars().EnableVectorizedExpression,
//                 &self.Filters,
//                 self.childChunkIter.as_mut().unwrap(),
//                 self.filtersSelected.clone(),
//             )?;
//             self.groupChecker
//                 .as_mut()
//                 .unwrap()
//                 .SplitIntoGroups(self.childChunk.as_ref().unwrap())?;
//         }
//         self.selectNextGroup();
//         self.groupRowsIter.Begin();
//         Ok(())
//     }
// hasNullInJoinKey 对应 Go 方法：只要任意 join key 为 NULL，inner group 就会被跳过。
//     pub fn hasNullInJoinKey(&self, row: chunk::Row) -> bool {
//         for col in &self.JoinKeys {
//             let ordinal = col.Index;
//             if row.IsNull(ordinal) {
//                 return true;
//             }
//         }
//         false
//     }
// }
// impl MergeJoinExec {
// Close implements the Executor Close interface.
//     pub fn Close(&mut self) -> Result<(), errors::Error> {
//         self.InnerTable.finish()?;
//         self.OuterTable.finish()?;
//         self.hasMatch = false;
//         self.hasNull = false;
//         self.memTracker = None;
//         self.diskTracker = None;
//         self.BaseExecutor.Close()
//     }
// Open implements the Executor Open interface.
//     pub fn Open(&mut self, ctx: context::Context) -> Result<(), errors::Error> {
//         self.BaseExecutor.Open(ctx)?;
//         self.memTracker = Some(memory::NewTracker(self.ID(), -1));
//         self.memTracker
//             .as_mut()
//             .unwrap()
//             .AttachTo(self.Ctx().GetSessionVars().StmtCtx.MemTracker);
//         self.diskTracker = Some(disk::NewTracker(self.ID(), -1));
//         self.diskTracker
//             .as_mut()
//             .unwrap()
//             .AttachTo(self.Ctx().GetSessionVars().StmtCtx.DiskTracker);
//         self.InnerTable.init(self);
//         self.OuterTable.init(self);
//         Ok(())
//     }
// Next implements the Executor Next interface.
// Note the inner group collects all identical keys in a group across multiple chunks, but the outer group just covers
// the identical keys within a chunk, so identical keys may cover more than one chunk.
//     pub fn Next(&mut self, ctx: context::Context, req: &mut chunk::Chunk) -> Result<(), errors::Error> {
//         req.Reset();
//         let mut innerIter = self.InnerTable.groupRowsIter.clone();
//         let mut outerIter = self.OuterTable.groupRowsIter.clone();
//         while !req.IsFull() {
//             failpoint::Inject("ConsumeRandomPanic", |_| {});
//             if innerIter.Current() == innerIter.End() {
//                 self.InnerTable.fetchNextInnerGroup(ctx.clone(), self)?;
//                 innerIter = self.InnerTable.groupRowsIter.clone();
//             }
//             if outerIter.Current() == outerIter.End() {
//                 self.OuterTable
//                     .fetchNextOuterGroup(ctx.clone(), self, req.RequiredRows() - req.NumRows())?;
//                 outerIter = self.OuterTable.groupRowsIter.clone();
//                 if self.OuterTable.executed {
//                     return Ok(());
//                 }
//             }
//             let mut cmpResult = if self.Desc { 1 } else { -1 };
//             if innerIter.Current() != innerIter.End() {
//                 cmpResult = self.compare(outerIter.Current(), innerIter.Current())?;
//             }
// inner group 落后时直接耗尽 inner iterator，让下一轮加载新 inner group。
//             if (cmpResult > 0 && !self.Desc) || (cmpResult < 0 && self.Desc) {
//                 innerIter.ReachEnd();
//                 continue;
//             }
// outer group 落后时，outer 行按 miss match 输出。
//             if (cmpResult < 0 && !self.Desc) || (cmpResult > 0 && self.Desc) {
//                 let mut row = outerIter.Current();
//                 while row != outerIter.End() && !req.IsFull() {
//                     self.Joiner.OnMissMatch(false, row, req);
//                     row = outerIter.Next();
//                 }
//                 continue;
//             }
//             let mut row = outerIter.Current();
//             while row != outerIter.End() && !req.IsFull() {
//                 if !self.OuterTable.filtersSelected[row.Idx()] {
//                     self.Joiner.OnMissMatch(false, row, req);
//                     row = outerIter.Next();
//                     continue;
//                 }
// 对当前 outer row 反复消费 inner group；chunk 满时保留 iterator 位置给下一次 Next。
//                 while innerIter.Current() != innerIter.End() {
//                     let (matched, isNull) = self.Joiner.TryToMatchInners(row, &mut innerIter, req)?;
//                     self.hasMatch = self.hasMatch || matched;
//                     self.hasNull = self.hasNull || isNull;
//                     if req.IsFull() {
//                         if innerIter.Current() == innerIter.End() {
//                             break;
//                         }
//                         return Ok(());
//                     }
//                 }
//                 if !self.hasMatch {
//                     self.Joiner.OnMissMatch(self.hasNull, row, req);
//                 }
//                 self.hasMatch = false;
//                 self.hasNull = false;
//                 innerIter.Begin();
//                 row = outerIter.Next();
//             }
//         }
//         Ok(())
//     }
// compare 对应 Go 方法：逐个 join key 调用 CompareFunc，第一个非零比较结果即为 group 顺序。
//     pub fn compare(&self, outerRow: chunk::Row, innerRow: chunk::Row) -> Result<i32, errors::Error> {
//         let outerJoinKeys = &self.OuterTable.JoinKeys;
//         let innerJoinKeys = &self.InnerTable.JoinKeys;
//         for i in 0..outerJoinKeys.len() {
//             let (cmp, _, err) = (self.CompareFuncs[i])(
//                 self.Ctx().GetExprCtx().GetEvalCtx(),
//                 &outerJoinKeys[i],
//                 &innerJoinKeys[i],
//                 outerRow,
//                 innerRow,
//             );
//             if let Some(err) = err {
//                 return Err(err);
//             }
//             if cmp != 0 {
//                 return Ok(cmp as i32);
//             }
//         }
//         Ok(0)
//     }
// }
// */
use crate::index_lookup_join::{compare_row, extract_key};
use crate::joiner::{Joiner, NaajType, Row};
use crate::row_table_builder::Value;

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
    closed: bool,
}

impl MergeJoinExec {
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
            closed: false,
        })
    }
    /// 打开执行器并初始化两侧表游标。
    pub fn open(&mut self) -> Result<(), String> {
        if self.closed {
            return Err("cannot reopen closed merge join".into());
        }
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
        if !self.opened {
            self.open()?;
        }
        if self.output.is_empty() && self.cursor == 0 {
            self.execute()?;
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
