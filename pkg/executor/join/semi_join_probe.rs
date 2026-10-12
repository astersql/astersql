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

// Semi Join（半连接）探测侧实现。
//
// 半连接只判断外表行是否在内表存在匹配，命中时输出外表行本身（不拼接内表列）。
// 本文件实现 `Probe`：按 build 侧在左/右分派匹配；左侧 build 时还要扫描 row table
// 输出已被标记使用的 build 行。

use crate::base_join_probe::{Probe, WorkerResult};
use crate::base_semi_join::BaseSemiJoin;
use crate::joiner::NaajType;
/// Semi Join 探测器：嵌入 `BaseSemiJoin`，实现哈希连接探测接口。
#[derive(Clone)]
pub struct SemiJoinProbe {
    /// 半连接共享状态（匹配标记、未完成队列、build 侧标志等）。
    pub semi: BaseSemiJoin,
}
impl Probe for SemiJoinProbe {
    /// 设置本轮 probe chunk，并重置半连接探测状态。
    fn set_chunk_for_probe(&mut self, chunk: Vec<crate::joiner::Row>) -> Result<(), String> {
        self.semi.base.set_chunk_for_probe(chunk)?;
        self.semi.reset_probe_state();
        Ok(())
    }
    /// Spill 恢复后的 probe chunk 设置；语义与 `set_chunk_for_probe` 相同。
    fn set_restored_chunk_for_probe(
        &mut self,
        chunk: Vec<crate::joiner::Row>,
    ) -> Result<(), String> {
        self.set_chunk_for_probe(chunk)
    }
    /// 把尚未探测完的 probe chunk 溢写出去，供后续恢复。
    fn spill_remaining_probe_chunks(&mut self) -> Vec<Vec<crate::joiner::Row>> {
        self.semi.base.spill_remaining_probe_chunks()
    }
    /// 主探测入口：在 chunk 容量内逐行匹配，左侧 build 标记 used，右侧 build 产出命中行。
    fn probe(&mut self) -> WorkerResult {
        let mut rows = Vec::new();
        // 在 max_chunk_size 限制下推进 current_probe_row。
        while self.semi.base.current_probe_row < self.semi.base.probe_chunk.len()
            && rows.len() < self.semi.base.context.max_chunk_size
        {
            let index = self.semi.base.current_probe_row;
            if self.semi.is_left_side_build {
                // 左侧为 build：只标记命中的 build 行，本阶段不直接输出。
                let build_indices = self.semi.base.matched_rows[index].clone();
                let build_rows: Vec<_> = build_indices
                    .iter()
                    .map(|build| self.semi.base.context.build_rows[*build].clone())
                    .collect();
                match self.semi.base.context.joiner.try_to_match_outers(
                    &build_rows,
                    &self.semi.base.probe_chunk[index],
                    &mut Vec::new(),
                ) {
                    Ok(statuses) => {
                        for (position, status) in statuses.iter().enumerate() {
                            if *status == crate::joiner::OuterRowStatus::Matched {
                                self.semi.base.context.build_row_used[build_indices[position]] =
                                    true;
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
            } else if let Err(error) =
                // 右侧为 build：命中则把 probe 行写入结果。
                self.semi
                        .match_probe_row(index, &mut rows, NaajType::Unknown)
            {
                return WorkerResult {
                    rows,
                    error: Some(error),
                };
            }
            self.semi.base.finish_current_lookup_loop();
        }
        WorkerResult { rows, error: None }
    }
    /// 仅左侧 build 的半连接需要在探测后再扫描 row table。
    fn need_scan_row_table(&self) -> bool {
        self.semi.is_left_side_build
    }
    /// 初始化 row table 扫描游标。
    fn init_for_scan_row_table(&mut self) {
        if !self.semi.is_left_side_build {
            panic!("should not reach here");
        }
        self.semi.base.scan_row_index = 0;
    }
    /// 扫描已被标记使用的 build 行并输出（半连接结果）。
    fn scan_row_table(&mut self) -> WorkerResult {
        if !self.semi.is_left_side_build {
            panic!("should not reach here");
        }
        self.semi.scan_build_rows(true)
    }
    /// row table 扫描是否已到末尾。
    fn is_scan_row_table_done(&self) -> bool {
        if !self.semi.is_left_side_build {
            panic!("should not reach here");
        }
        self.semi.base.scan_row_index >= self.semi.base.context.build_rows.len()
    }
    /// 当前 probe chunk 是否处理完毕（含 other condition 未完成队列为空）。
    fn is_current_chunk_probe_done(&self) -> bool {
        self.semi.unfinished_probe_rows.is_empty() && self.semi.base.is_current_chunk_probe_done()
    }
    /// 重置探测状态，准备下一轮。
    fn reset_probe(&mut self) {
        self.semi.base.reset_probe();
        self.semi.reset_probe_state();
    }
}
