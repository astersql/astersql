// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// LIKE 的向量化求值实现。
//
// 对应 Go `builtin_like_vec.go`：对 Chunk 中整列字符串做通配匹配，结果写入 Int64 列。
// 与标量路径不同，向量化路径故意不使用 `pattern_cache`，避免模式按行变化时的数据竞争。

use crate::builtin_like_kernel::builtinLikeSig;
use crate::legacy_vectorized_runtime::{Chunk, Column, EvalContext, Result};

impl builtinLikeSig {
    /// LIKE 支持向量化求值。
    pub fn vectorized(&self) -> bool {
        true
    }

    /// 按行编译模式并匹配；合并三列 NULL；匹配结果以 0/1 写入 Int64。
    pub fn vecEvalInt(&self, ctx: &EvalContext, input: &Chunk, result: &mut Column) -> Result<()> {
        let rows = input.NumRows();
        let mut values = Column::default();
        self.args[0].VecEvalString(ctx, input, &mut values)?;
        let mut patterns = Column::default();
        self.args[1].VecEvalString(ctx, input, &mut patterns)?;
        let mut escapes = Column::default();
        self.args[2].VecEvalInt(ctx, input, &mut escapes)?;

        // Do not use pattern_cache here: the Go vectorized path deliberately
        // 本地匹配器：避免按行变化的模式与共享缓存产生竞争。
        // owns a local matcher to avoid races while patterns change per row.
        let mut pattern = self.collator.Pattern();
        result.ResizeInt64(rows, false);
        result.MergeNulls(&[&values, &patterns, &escapes]);
        for row in 0..rows {
            if result.IsNull(row) {
                continue;
            }
            pattern.Compile(patterns.GetString(row), escapes.Int64s()[row] as u8);
            result.Int64sMut()[row] = i64::from(pattern.DoMatch(values.GetString(row)));
        }
        Ok(())
    }
}
