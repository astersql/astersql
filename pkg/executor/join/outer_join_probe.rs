// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Outer Join（外连接）Hash Join probe 实现。
//
// Outer Join 保证 outer 侧每一行至少输出一次：有匹配则拼 inner 列，无匹配则为
// inner 侧补 NULL（默认值）。当 outer 侧作为 build 时，probe 结束后还需扫描
// row table 补出未匹配的 build 行。对应 Go 的 `outerJoinProbe`。

use crate::base_join_probe::{BaseJoinProbe, Probe, WorkerResult};
use crate::joiner::{NaajType, OuterRowStatus};

/// Left / Right Outer Join 共用的 probe 状态。
///
/// `outer_side_build` 为 true 表示 outer 侧建哈希表，probe 后需扫描未使用 build 行；
/// `right_side_build` 记录右表是否为 build 侧，影响结果列顺序。
#[derive(Clone)]
pub struct OuterJoinProbe {
    /// 公共 probe 基座（chunk、匹配链表、上下文）。
    pub base: BaseJoinProbe,
    /// outer 侧是否作为 build 侧。
    pub outer_side_build: bool,
    /// 右表是否作为 build 侧。
    pub right_side_build: bool,
}

impl Probe for OuterJoinProbe {
    /// 设置本轮 probe chunk。
    fn set_chunk_for_probe(&mut self, chunk: Vec<crate::joiner::Row>) -> Result<(), String> {
        self.base.set_chunk_for_probe(chunk)
    }
    /// 设置 spill 恢复后的 probe chunk。
    fn set_restored_chunk_for_probe(
        &mut self,
        chunk: Vec<crate::joiner::Row>,
    ) -> Result<(), String> {
        self.base.set_restored_chunk_for_probe(chunk)
    }
    /// 溢出尚未探测完的 probe 行。
    fn spill_remaining_probe_chunks(&mut self) -> Vec<Vec<crate::joiner::Row>> {
        self.base.spill_remaining_probe_chunks()
    }
    /// 按 outer/inner build 两条路径探测并产出连接结果。
    fn probe(&mut self) -> WorkerResult {
        let mut rows = Vec::new();
        // 在输出容量允许时推进当前 probe 行。
        while self.base.current_probe_row < self.base.probe_chunk.len()
            && rows.len() < self.base.context.max_chunk_size
        {
            let index = self.base.current_probe_row;
            let probe_row = self.base.probe_chunk[index].clone();
            let candidate_start = self.base.current_candidate;
            let candidate_end = (candidate_start
                + (self.base.context.max_chunk_size - rows.len()).max(1))
            .min(self.base.matched_rows[index].len());
            let candidate_complete = candidate_end == self.base.matched_rows[index].len();
            let build_indices =
                self.base.matched_rows[index][candidate_start..candidate_end].to_vec();
            let build_rows: Vec<_> = build_indices
                .iter()
                .map(|build| self.base.context.build_rows[*build].clone())
                .collect();
            if self.outer_side_build {
                // Outer build：以 build 行为 outer，匹配成功则标记 used。
                match self.base.context.joiner.try_to_match_outers(
                    &build_rows,
                    &probe_row,
                    &mut rows,
                ) {
                    Ok(statuses) => {
                        for (position, status) in statuses.iter().enumerate() {
                            if *status == OuterRowStatus::Matched {
                                self.base.context.build_row_used[build_indices[position]] = true;
                            }
                        }
                    }
                    Err(error) => {
                        return WorkerResult {
                            rows,
                            error: Some(error),
                        };
                    }
                }
                self.base.current_candidate += build_indices.len();
            } else {
                // Inner build：probe 行为 outer，未匹配时补默认 inner 行。
                match self.base.context.joiner.try_to_match_inners(
                    &probe_row,
                    &build_rows,
                    &mut rows,
                    NaajType::Unknown,
                ) {
                    Ok(result) => {
                        if result.matched {
                            self.base.mark_build_rows_used(index);
                        } else if candidate_complete {
                            self.base.context.joiner.on_miss_match(
                                result.has_null,
                                &probe_row,
                                &mut rows,
                            );
                        }
                    }
                    Err(error) => {
                        return WorkerResult {
                            rows,
                            error: Some(error),
                        };
                    }
                }
                self.base.current_candidate += build_indices.len();
            }
            if self.base.current_candidate >= self.base.matched_rows[index].len() {
                self.base.finish_current_lookup_loop();
            }
        }
        WorkerResult { rows, error: None }
    }
    /// 仅 outer 侧 build 时需要扫描未匹配的 build 行。
    fn need_scan_row_table(&self) -> bool {
        self.outer_side_build
    }
    /// 初始化 build row table 扫描游标。
    fn init_for_scan_row_table(&mut self) {
        assert!(self.outer_side_build, "should not reach here");
        self.base.scan_row_index = 0;
    }
    /// 扫描未使用的 build 行，为 probe 侧补默认值后输出。
    fn scan_row_table(&mut self) -> WorkerResult {
        assert!(self.outer_side_build, "should not reach here");
        let mut rows = Vec::new();
        while self.base.scan_row_index < self.base.context.build_rows.len()
            && rows.len() < self.base.context.max_chunk_size
        {
            let index = self.base.scan_row_index;
            self.base.scan_row_index += 1;
            if !self.base.context.build_row_used[index] {
                let build = self.base.context.build_rows[index].clone();
                self.base
                    .context
                    .joiner
                    .on_miss_match(false, &build, &mut rows);
            }
        }
        WorkerResult { rows, error: None }
    }
    /// 是否已扫完全部 build 行。
    fn is_scan_row_table_done(&self) -> bool {
        assert!(self.outer_side_build, "should not reach here");
        self.base.scan_row_index >= self.base.context.build_rows.len()
    }
    fn is_current_chunk_probe_done(&self) -> bool {
        self.base.is_current_chunk_probe_done()
    }
    fn reset_probe(&mut self) {
        self.base.reset_probe();
    }
}
