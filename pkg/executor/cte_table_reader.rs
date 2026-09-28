// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// CTE（Common Table Expression，公用表表达式）结果表读取执行器。
//
// 递归 CTE 由 `CTEExec` 写入迭代存储；本模块的 `CTETableReaderExec`
// 作为消费者按 chunk（列式数据块）扫描该存储，供上层算子复用中间结果。

#![allow(non_snake_case)]

use astersql_util_chunk::Chunk;
use astersql_util_cteutil::{Storage, errors};

/// 执行器框架提供的生命周期钩子。
///
/// 以 trait 抽象 `Open`/`Close`，便于在 `BaseExecutor` 尚未完全移植时接入。
/// Lifecycle supplied by the executor framework. Keeping this as a trait makes
/// the CTE reader usable while `internal/exec::BaseExecutor` is being ported.
pub trait CTEReaderBase {
    fn open(&mut self) -> Result<(), errors::Error>;
    fn close(&mut self) -> Result<(), errors::Error>;
}

/// 扫描 `iter_in_tbl` 中的数据；该表由对应的 `CTEExec` 填充。
///
/// - `chk_idx`：当前 chunk 下标；
/// - `cur_iter`：当前 CTE 迭代轮次，与存储层迭代计数对齐。
/// Scans data in `iter_in_tbl`, which is filled by the corresponding CTEExec.
pub struct CTETableReaderExec<B: CTEReaderBase> {
    pub base_executor: B,
    pub iter_in_tbl: Box<dyn Storage>,
    pub chk_idx: usize,
    pub cur_iter: isize,
}

impl<B: CTEReaderBase> CTETableReaderExec<B> {
    /// 打开读取器：复位游标并打开底层执行器。
    pub fn Open<C>(&mut self, _ctx: C) -> Result<(), errors::Error> {
        self.reset();
        self.base_executor.open()
    }

    /// 拉取下一个 chunk；迭代轮次变化时从 chunk 0 重新扫描。
    pub fn Next<C>(&mut self, _ctx: C, req: &mut Chunk) -> Result<(), errors::Error> {
        req.Reset();

        // Upstream operators may consume the entire storage in a loop, so the
        // iteration counter—not the chunk index—defines a new CTE iteration.
        // 上游可能循环耗尽存储，因此以迭代计数（而非 chunk 下标）判定新一轮 CTE 迭代。
        let storage_iter = self.iter_in_tbl.GetIter();
        if self.cur_iter != storage_iter {
            // 读者超前于生产者属于非法状态
            if self.cur_iter > storage_iter {
                return Err(errors::New(format!(
                    "invalid iteration for CTETableReaderExec (e.curIter: {}, e.iterInTbl.GetIter(): {})",
                    self.cur_iter, storage_iter
                )));
            }
            self.chk_idx = 0;
            self.cur_iter = storage_iter;
        }

        if self.chk_idx < self.iter_in_tbl.NumChunks() {
            let source = self.iter_in_tbl.GetChunk(self.chk_idx)?;
            // Do not let an upper executor mutate the shared CTE storage.
            // 拷贝列数据，避免上层算子修改共享的 CTE 存储。
            let mut copy = source.CopyConstructSel();
            req.SwapColumns(&mut copy);
            self.chk_idx += 1;
        }
        Ok(())
    }

    /// 关闭读取器：复位游标并关闭底层执行器。
    pub fn Close(&mut self) -> Result<(), errors::Error> {
        self.reset();
        self.base_executor.close()
    }

    /// 将 chunk 下标与迭代计数归零。
    pub fn reset(&mut self) {
        self.chk_idx = 0;
        self.cur_iter = 0;
    }
}
