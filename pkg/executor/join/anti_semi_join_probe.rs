// Copyright 2026 AsterSQL.
// Anti Semi Join（反半连接）的 Probe 实现。
//
// Anti Semi Join：仅当左侧行在右侧找不到匹配时才输出该左侧行（类似 `NOT EXISTS` /
// `NOT IN`）。本文件在 Hash Join 的 Probe 阶段实现该语义：右侧为 build side 时
// 输出未匹配 probe 行；左侧为 build side 时在扫描 build 表阶段输出未使用的 build 行。

use crate::base_join_probe::{Probe, WorkerResult};
use crate::base_semi_join::BaseSemiJoin;
use crate::joiner::NaajType;
#[derive(Clone)]
/// Anti Semi Join 的 Probe 执行器；内嵌 `BaseSemiJoin` 复用半连接共享状态。
pub struct AntiSemiJoinProbe {
    /// 半连接共用状态（匹配标记、未完成队列、build side 标记等）。
    pub semi: BaseSemiJoin,
}
impl Probe for AntiSemiJoinProbe {
    /// 设置本批 probe Chunk，并重置 anti semi 匹配状态。
    fn set_chunk_for_probe(&mut self, chunk: Vec<crate::joiner::Row>) -> Result<(), String> {
        self.semi.base.set_chunk_for_probe(chunk)?;
        self.semi.reset_probe_state();
        Ok(())
    }
    /// 从 spill 恢复的 Chunk 走与普通 probe 相同的预处理路径。
    fn set_restored_chunk_for_probe(
        &mut self,
        chunk: Vec<crate::joiner::Row>,
    ) -> Result<(), String> {
        self.set_chunk_for_probe(chunk)
    }
    /// 将尚未处理完的 probe 行刷出为 spill Chunk 列表。
    fn spill_remaining_probe_chunks(&mut self) -> Vec<Vec<crate::joiner::Row>> {
        self.semi.base.spill_remaining_probe_chunks()
    }
    /// 执行 anti semi probe：按 build side 分支处理匹配或输出未匹配行。
    fn probe(&mut self) -> WorkerResult {
        let mut rows = Vec::new();
        // 在输出容量用尽前逐行推进 current_probe_row。
        while self.semi.base.current_probe_row < self.semi.base.probe_chunk.len()
            && rows.len() < self.semi.base.context.max_chunk_size
        {
            let index = self.semi.base.current_probe_row;
            if self.semi.is_left_side_build {
                // 左侧 build：在 probe 阶段只标记被命中的 build 行，输出留到 scan_row_table。
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
                            if matches!(
                                status,
                                crate::joiner::OuterRowStatus::Matched
                                    | crate::joiner::OuterRowStatus::HasNull
                            ) {
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
            } else {
                // 右侧 build：未匹配的 probe 行通过 on_miss_match 写入结果。
                match self
                    .semi
                    .match_probe_row(index, &mut Vec::new(), NaajType::Unknown)
                {
                    Ok(result) => {
                        if !result.matched {
                            self.semi.base.context.joiner.on_miss_match(
                                result.has_null,
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
            }
            self.semi.base.finish_current_lookup_loop();
        }
        WorkerResult { rows, error: None }
    }
    /// 仅左侧作为 build side 时需要事后扫描未使用的 build 行。
    fn need_scan_row_table(&self) -> bool {
        self.semi.is_left_side_build
    }
    /// 初始化 build 表扫描游标。
    fn init_for_scan_row_table(&mut self) {
        self.semi.base.scan_row_index = 0;
    }
    /// 扫描并输出未被 probe 命中的 build 行（anti 语义下 expected_used=false）。
    fn scan_row_table(&mut self) -> WorkerResult {
        self.semi.scan_build_rows(false)
    }
    /// build 表扫描是否已结束。
    fn is_scan_row_table_done(&self) -> bool {
        self.semi.base.scan_row_index >= self.semi.base.context.build_rows.len()
    }
    /// 当前 probe Chunk 是否处理完毕（含 other condition 未完成队列为空）。
    fn is_current_chunk_probe_done(&self) -> bool {
        self.semi.unfinished_probe_rows.is_empty() && self.semi.base.is_current_chunk_probe_done()
    }
    /// 重置 probe 与 anti semi 共享状态，准备下一批。
    fn reset_probe(&mut self) {
        self.semi.base.reset_probe();
        self.semi.reset_probe_state();
    }
}
