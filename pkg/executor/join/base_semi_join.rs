// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// Semi / Anti Semi Join 共享的 Probe 基类。
//
// Semi Join（半连接）只输出在对侧存在匹配的左侧行，不展开笛卡尔积；
// Anti Semi 则输出无匹配的左侧行。本模块提供匹配标记、未完成 probe 队列，
// 以及扫描 build 行等共用逻辑。

use crate::base_join_probe::{BaseJoinProbe, WorkerResult};
use crate::joiner::{MatchResult, NaajType, Row};
use std::collections::VecDeque;

/// 存在 other condition 时，单次中间 joined chunk 允许为每个 probe 行匹配的最大 build 行数。
/// 用于限制中间结果膨胀（对应 Go 的 maxMatchedRowNum）。
pub const MAX_MATCHED_ROW_NUM: usize = 4;
#[derive(Clone)]
/// Semi / Anti Semi 共用状态：嵌入 BaseJoinProbe，并记录 build side 与匹配信息。
pub struct BaseSemiJoin {
    /// 底层 Probe 状态（哈希查找、游标等）。
    pub base: BaseJoinProbe,
    /// 是否以左侧作为 build side。
    pub is_left_side_build: bool,
    /// 各 probe 行是否已找到匹配（右侧 build 时使用）。
    pub matched: Vec<bool>,
    /// 各 probe 行匹配过程中是否遇到过 NULL（影响 NULL-aware 语义）。
    pub has_null: Vec<bool>,
    /// 因容量限制尚未处理完的 probe 行下标队列。
    pub unfinished_probe_rows: VecDeque<usize>,
}
impl BaseSemiJoin {
    /// 用 BaseJoinProbe 与 build side 标记构造共享状态。
    pub fn new(base: BaseJoinProbe, is_left_side_build: bool) -> Self {
        Self {
            base,
            is_left_side_build,
            matched: Vec::new(),
            has_null: Vec::new(),
            unfinished_probe_rows: VecDeque::new(),
        }
    }
    /// 按当前 probe Chunk 行数重建 matched / has_null，并清空未完成队列。
    pub fn reset_probe_state(&mut self) {
        let count = self.base.probe_chunk.len();
        self.matched.clear();
        self.matched.resize(count, false);
        self.has_null.clear();
        self.has_null.resize(count, false);
        self.unfinished_probe_rows.clear();
    }
    /// 对指定 probe 行与其候选 build 行做匹配，更新 matched/has_null，必要时入队未完成行。
    pub fn match_probe_row(
        &mut self,
        probe_index: usize,
        output: &mut Vec<Row>,
        naaj: NaajType,
    ) -> Result<MatchResult, String> {
        let outer = self.base.probe_chunk[probe_index].clone();
        let inners = self.base.candidate_rows(probe_index);
        let result = self
            .base
            .context
            .joiner
            .try_to_match_inners(&outer, &inners, output, naaj)?;
        self.matched[probe_index] |= result.matched;
        self.has_null[probe_index] |= result.has_null;
        // 候选未全部消费完时，稍后继续处理该 probe 行。
        if result.consumed < inners.len() {
            self.unfinished_probe_rows.push_back(probe_index);
        }
        Ok(result)
    }
    /// 将 matched 标志等于 expected_match 的 probe 行交给 Joiner.on_miss_match 输出。
    pub fn produce_probe_mismatches(&self, expected_match: bool, output: &mut Vec<Row>) {
        for (index, matched) in self.matched.iter().enumerate() {
            if *matched == expected_match {
                self.base.context.joiner.on_miss_match(
                    self.has_null[index],
                    &self.base.probe_chunk[index],
                    output,
                );
            }
        }
    }
    /// 扫描 build 行表，输出 used 标记等于 expected_used 的行（容量受 max_chunk_size 限制）。
    pub fn scan_build_rows(&mut self, expected_used: bool) -> WorkerResult {
        let mut rows = Vec::new();
        while self.base.scan_row_index < self.base.context.build_rows.len()
            && rows.len() < self.base.context.max_chunk_size
        {
            let index = self.base.scan_row_index;
            self.base.scan_row_index += 1;
            if self.base.context.build_row_used[index] == expected_used {
                rows.push(self.base.context.build_rows[index].clone());
            }
        }
        WorkerResult { rows, error: None }
    }
}
