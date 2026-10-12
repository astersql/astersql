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

// Hash Join V2 的 Inner Join（内连接）探测（probe）实现。
//
// `InnerJoinProbe` 嵌入 `BaseJoinProbe`：按探测侧行查找构建侧候选，
// 经 Joiner 做 key 匹配与 other condition（附加条件）过滤后写出结果。
// Inner join 不需要 probe 结束后扫描整张 row table。

use crate::base_join_probe::{BaseJoinProbe, Probe, WorkerResult};
use crate::joiner::NaajType;
/// Inner Join 探测实现：委托 `BaseJoinProbe` 管理探测 chunk 与候选行。
#[derive(Clone)]
pub struct InnerJoinProbe {
    /// 共享探测状态（当前行、碰撞计数、Joiner 上下文等）。
    pub base: BaseJoinProbe,
}
impl Probe for InnerJoinProbe {
    fn set_chunk_for_probe(&mut self, chunk: Vec<crate::joiner::Row>) -> Result<(), String> {
        self.base.set_chunk_for_probe(chunk)
    }
    fn set_restored_chunk_for_probe(
        &mut self,
        chunk: Vec<crate::joiner::Row>,
    ) -> Result<(), String> {
        self.base.set_restored_chunk_for_probe(chunk)
    }
    fn spill_remaining_probe_chunks(&mut self) -> Vec<Vec<crate::joiner::Row>> {
        self.base.spill_remaining_probe_chunks()
    }
    /// 在结果未满时逐行探测：取候选内表行，经 Joiner 匹配后推进探测游标。
    fn probe(&mut self) -> WorkerResult {
        let mut rows = Vec::new();
        // 结果 chunk 容量未满且当前探测 chunk 未完成时持续匹配。
        while !self.base.is_current_chunk_probe_done()
            && rows.len() < self.base.context.max_chunk_size
        {
            let index = self.base.current_probe_row;
            let outer = self.base.probe_chunk[index].clone();
            let candidate_start = self.base.current_candidate;
            let candidate_end = (candidate_start
                + (self.base.context.max_chunk_size - rows.len()).max(1))
            .min(self.base.matched_rows[index].len());
            let inners: Vec<_> = self.base.matched_rows[index][candidate_start..candidate_end]
                .iter()
                .map(|build| self.base.context.build_rows[*build].clone())
                .collect();
            match self.base.context.joiner.try_to_match_inners(
                &outer,
                &inners,
                &mut rows,
                NaajType::Unknown,
            ) {
                Ok(result) => {
                    // 未真正 key 命中的候选计为 probe collision。
                    self.base.probe_collision +=
                        inners.len().saturating_sub(usize::from(result.matched)) as u64;
                    self.base.current_candidate += result.consumed;
                    if result.matched {
                        self.base.mark_build_rows_used(index);
                    }
                    if self.base.current_candidate >= self.base.matched_rows[index].len() {
                        self.base.finish_current_lookup_loop();
                    }
                }
                Err(error) => {
                    return WorkerResult {
                        rows,
                        error: Some(error),
                    };
                }
            }
        }
        WorkerResult { rows, error: None }
    }
    /// Inner join 不需要在探测结束后扫描整张构建侧 row table。
    fn need_scan_row_table(&self) -> bool {
        false
    }
    fn init_for_scan_row_table(&mut self) {
        panic!("should not reach here");
    }
    /// Inner join 不应走扫描路径，与 Go 一样视为不可达。
    fn scan_row_table(&mut self) -> WorkerResult {
        panic!("should not reach here");
    }
    fn is_scan_row_table_done(&self) -> bool {
        panic!("should not reach here");
    }
    fn is_current_chunk_probe_done(&self) -> bool {
        self.base.is_current_chunk_probe_done()
    }
    fn reset_probe(&mut self) {
        self.base.reset_probe();
    }
}
