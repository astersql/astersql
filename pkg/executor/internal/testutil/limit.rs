// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// Limit 算子测试用例定义。
//
// Limit 对应 SQL 的 `LIMIT offset, count`：跳过前 offset 行后最多返回 count 行；
// 可配合列裁剪（projection）只保留子节点 schema 中被使用的列。

use crate::testutil::{ColumnDef, FieldKind, MemoryTracker, SessionContext, resetChunkSizes};
use std::fmt::{Display, Formatter};

/// Limit 算子单次测试的参数集合。
pub struct LimitCase {
    /// 会话上下文。
    pub Ctx: SessionContext,
    /// 子节点各列是否被上层使用（用于 inline projection / 列裁剪）。
    pub ChildUsedSchema: Vec<bool>,
    /// 子节点输出总行数。
    pub Rows: usize,
    /// 跳过的行数（SQL OFFSET）。
    pub Offset: usize,
    /// 最多返回的行数（SQL LIMIT count）。
    pub Count: usize,
    /// 是否启用内联投影（在 Limit 内直接裁剪列，避免额外 Projection 算子）。
    pub UsingInlineProjection: bool,
}
impl LimitCase {
    /// 返回用例默认两列 LongLong 的列定义。
    pub fn Columns(&self) -> Vec<ColumnDef> {
        vec![
            ColumnDef::new(0, FieldKind::LongLong),
            ColumnDef::new(1, FieldKind::LongLong),
        ]
    }
}
impl Display for LimitCase {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "(rows:{}, offset:{}, count:{}, inline_projection:{})",
            self.Rows, self.Offset, self.Count, self.UsingInlineProjection
        )
    }
}
/// 构造默认 Limit 用例：3 万行输入，OFFSET 1 万、COUNT 1 万，关闭语句内存限额。
pub fn DefaultLimitTestCase(mut ctx: SessionContext) -> LimitCase {
    resetChunkSizes(&mut ctx);
    // limit=-1 表示不限制语句级内存。
    ctx.vars.statement_memory_tracker = MemoryTracker {
        limit: -1,
        attached: false,
    };
    LimitCase {
        Ctx: ctx,
        Rows: 30_000,
        Offset: 10_000,
        Count: 10_000,
        ChildUsedSchema: vec![false, true],
        UsingInlineProjection: false,
    }
}
