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

// Left Outer Semi / Anti Semi Join 的 Hash Join probe 实现。
//
// Left Outer Semi Join（左外半连接）为左表每一行输出匹配标记（true/false/NULL），
// 而不是展开右表列；Anti 变体对匹配结果取反。null-aware（空值感知）路径会按
// `NaajType` 处理三值逻辑。对应 Go 的 `leftOuterSemiJoinProbe`。

use crate::base_join_probe::{Probe, WorkerResult};
use crate::base_semi_join::BaseSemiJoin;
use crate::joiner::NaajType;

/// Left Outer Semi / Anti Semi Join 的 probe 状态。
///
/// 嵌入 `BaseSemiJoin` 复用匹配队列与标记列输出；`anti` 决定标记是否取反，
/// `null_aware` 启用空值感知半连接（NULL-Aware Anti/Semi Join）路径。
#[derive(Clone)]
pub struct LeftOuterSemiJoinProbe {
    /// Semi Join 共享基座（含 base probe 与匹配状态）。
    pub semi: BaseSemiJoin,
    /// 为 true 时按 Anti Semi 语义输出匹配标记。
    pub anti: bool,
    /// 为 true 时按 null-aware 三值逻辑探测。
    pub null_aware: bool,
}

impl Probe for LeftOuterSemiJoinProbe {
    /// 设置本轮 probe chunk，并重置 semi 匹配状态。
    fn set_chunk_for_probe(&mut self, chunk: Vec<crate::joiner::Row>) -> Result<(), String> {
        self.semi.base.set_chunk_for_probe(chunk)?;
        self.semi.reset_probe_state();
        Ok(())
    }
    /// Spill 恢复后的 probe chunk：复用 `set_chunk_for_probe`。
    fn set_restored_chunk_for_probe(
        &mut self,
        chunk: Vec<crate::joiner::Row>,
    ) -> Result<(), String> {
        self.set_chunk_for_probe(chunk)
    }
    /// 将尚未探测完的 probe 行整块 spill 出去。
    fn spill_remaining_probe_chunks(&mut self) -> Vec<Vec<crate::joiner::Row>> {
        self.semi.base.spill_remaining_probe_chunks()
    }
    /// 对当前 probe chunk 逐行匹配 build 侧，输出左行 + 标记列。
    fn probe(&mut self) -> WorkerResult {
        let mut rows = Vec::new();
        // 在输出 chunk 容量允许时持续推进当前 probe 行。
        while self.semi.base.current_probe_row < self.semi.base.probe_chunk.len()
            && rows.len() < self.semi.base.context.max_chunk_size
        {
            let index = self.semi.base.current_probe_row;
            let probe_key_has_null = self.semi.base.context.probe_key_indices.iter().any(|key| {
                matches!(
                    self.semi.base.probe_chunk[index].get(*key),
                    Some(crate::row_table_builder::Value::Null)
                )
            });
            let build_has_null_key =
                self.semi.base.context.build_rows.iter().any(|row| {
                    self.semi.base.context.build_key_indices.iter().any(|key| {
                        matches!(row.get(*key), Some(crate::row_table_builder::Value::Null))
                    })
                });
            // null-aware 路径传入具体 NaajType；否则走普通半连接匹配。
            let naaj = if self.null_aware {
                if probe_key_has_null {
                    NaajType::LeftHasNullRightNotNull
                } else {
                    NaajType::LeftNotNullRightNotNull
                }
            } else {
                NaajType::Unknown
            };
            match self.semi.match_probe_row(index, &mut rows, naaj) {
                Ok(result) => {
                    // 未匹配时由 Joiner 按 has_null 写出 false 或 NULL 标记。
                    if !result.matched {
                        let has_null = if self.null_aware {
                            // NAAJ 的 NULL 只来自 join key；inner filter 的 NULL 仍按未命中处理。
                            probe_key_has_null || build_has_null_key
                        } else {
                            result.has_null
                        };
                        self.semi.base.context.joiner.on_miss_match(
                            has_null,
                            &self.semi.base.probe_chunk[index],
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
            self.semi.base.finish_current_lookup_loop();
        }
        WorkerResult { rows, error: None }
    }
    /// Left Outer Semi 不需要扫描整张 build row table 补未匹配行。
    fn need_scan_row_table(&self) -> bool {
        false
    }
    fn init_for_scan_row_table(&mut self) {
        panic!("should not reach here")
    }
    fn scan_row_table(&mut self) -> WorkerResult {
        WorkerResult::default()
    }
    fn is_scan_row_table_done(&self) -> bool {
        panic!("should not reach here")
    }
    /// 有未完成 other-condition 队列或 base 未结束时，当前 chunk 仍需继续 probe。
    fn is_current_chunk_probe_done(&self) -> bool {
        self.semi.unfinished_probe_rows.is_empty() && self.semi.base.is_current_chunk_probe_done()
    }
    /// 重置 base 与 semi 两侧的 probe 状态，供下一轮复用。
    fn reset_probe(&mut self) {
        self.semi.base.reset_probe();
        self.semi.reset_probe_state();
    }
}
