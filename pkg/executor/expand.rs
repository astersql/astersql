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

// Expand 执行器：把一行按多个 grouping set（分组集合）展开为多行。
//
// 用于 `GROUPING SETS` / `ROLLUP` / `CUBE` 等场景：优化器把多组分组键
// 编码为若干 level，执行器对同一批子节点输入逐 level 求值并写出。
// 并行 Expand 在 Go 侧尚未完成求值顺序与内存记账，此处同样强制单线程。

#![allow(non_snake_case)]

use astersql_util_chunk::Chunk;

/// Expand 对运行时的依赖：子节点拉取、按 level 求值、内存与并发登记。
pub trait ExpandBackend {
    type Error;

    fn open(&mut self) -> Result<(), Self::Error>;
    fn close(&mut self) -> Result<(), Self::Error>;
    fn max_chunk_size(&self) -> usize;
    fn new_child_chunk(&mut self) -> Chunk;
    fn next_child<C>(&mut self, ctx: C, chunk: &mut Chunk) -> Result<(), Self::Error>;
    /// 对当前输入 chunk 执行第 `level` 组 grouping set 投影。
    fn run_level(
        &mut self,
        level: usize,
        input: &Chunk,
        output: &mut Chunk,
    ) -> Result<(), Self::Error>;
    fn consume_memory(&mut self, bytes: i64);
    fn reset_memory_tracker(&mut self);
    fn register_concurrency(&mut self, workers: i64);
    fn parallel_not_implemented(&self) -> Self::Error;
}

/// Expand 算子状态：缓存子节点 chunk，并在多个 level 间迭代。
pub struct ExpandExec<B: ExpandBackend> {
    pub backend: B,
    pub num_workers: i64,
    pub child_result: Option<Chunk>,
    /// 当前 level 下标；-1 表示需要先拉取下一批子节点行。
    pub level_iter_offset: isize,
    pub level_count: usize,
}

impl<B: ExpandBackend> ExpandExec<B> {
    /// 打开后端并初始化单线程 Expand 状态。
    pub fn Open<C>(&mut self, ctx: C) -> Result<(), B::Error> {
        self.backend.open()?;
        self.open(ctx)
    }

    /// 重置内存追踪；强制 `num_workers=0`（禁用并行路径）。
    pub fn open<C>(&mut self, _ctx: C) -> Result<(), B::Error> {
        self.backend.reset_memory_tracker();
        // Parallel Expand is intentionally disabled in Go until its evaluator
        // ordering and memory accounting are implemented.
        // 并行 Expand 在求值顺序与内存记账完成前故意关闭，与 Go 一致。
        self.num_workers = 0;
        if self.isUnparalleled() {
            self.level_iter_offset = -1;
            let child = self.backend.new_child_chunk();
            self.backend.consume_memory(child.MemoryUsage());
            self.child_result = Some(child);
        }
        Ok(())
    }

    /// 按是否并行分派到单线程或并行路径。
    pub fn Next<C>(&mut self, ctx: C, req: &mut Chunk) -> Result<(), B::Error> {
        req.GrowAndReset(self.backend.max_chunk_size());
        if self.isUnparalleled() {
            self.unParallelExecute(ctx, req)
        } else {
            self.parallelExecute(ctx, req)
        }
    }

    /// `num_workers <= 0` 表示走单线程 Expand。
    pub fn isUnparalleled(&self) -> bool {
        self.num_workers <= 0
    }

    /// 单线程：必要时拉子节点，再对当前 level 写出一行展开结果。
    pub fn unParallelExecute<C>(&mut self, ctx: C, output: &mut Chunk) -> Result<(), B::Error> {
        // level 用尽或尚未开始时，按 RequiredRows 拉取下一批子节点输入。
        if self.level_iter_offset == -1 || self.level_iter_offset as usize >= self.level_count {
            let child = self
                .child_result
                .as_mut()
                .expect("ExpandExec child chunk must be initialized by Open");
            child.SetRequiredRows(output.RequiredRows(), self.backend.max_chunk_size());
            let previous_memory = child.MemoryUsage();
            self.backend.next_child(ctx, child)?;
            self.backend
                .consume_memory(child.MemoryUsage() - previous_memory);
            if child.NumRows() == 0 {
                return Ok(());
            }
            self.level_iter_offset = 0;
        }

        let level = self.level_iter_offset as usize;
        let child = self
            .child_result
            .as_ref()
            .expect("ExpandExec child chunk must be initialized by Open");
        self.backend.run_level(level, child, output)?;
        self.level_iter_offset += 1;
        Ok(())
    }

    /// 并行路径尚未实现，直接返回后端错误。
    pub fn parallelExecute<C>(&mut self, _ctx: C, _output: &mut Chunk) -> Result<(), B::Error> {
        Err(self.backend.parallel_not_implemented())
    }

    /// 释放子 chunk 内存记账并关闭后端。
    pub fn Close(&mut self) -> Result<(), B::Error> {
        if self.isUnparalleled() {
            if let Some(child) = self.child_result.take() {
                self.backend.consume_memory(-child.MemoryUsage());
            }
        }
        self.backend.register_concurrency(self.num_workers.max(0));
        self.backend.close()
    }
}
