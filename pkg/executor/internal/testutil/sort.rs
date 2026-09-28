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

// Sort 算子测试用例定义。
//
// Sort 按 ORDER BY 列对输入行排序；内存不足时可 spill 到磁盘临时文件。
// NDV 控制各列不同值个数，用于构造有序/无序输入分布。

use crate::testutil::{ColumnDef, FieldKind, MemoryTracker, SessionContext, resetChunkSizes};
use std::fmt::{Display, Formatter};

/// Sort 算子单次测试的参数集合。
pub struct SortCase {
    /// 会话上下文（含内存跟踪，用于触发 spill）。
    pub Ctx: SessionContext,
    /// spill 临时文件名前缀（测试中用于泄漏检查）。
    pub FileNamePrefixForTest: String,
    /// ORDER BY 列下标列表。
    pub OrderByIdx: Vec<usize>,
    /// 各列 NDV（0 表示不限制/随机生成）。
    pub Ndvs: Vec<i32>,
    /// 输入总行数。
    pub Rows: usize,
}
impl SortCase {
    /// 返回用例默认两列 LongLong 的列定义。
    pub fn Columns(&self) -> Vec<ColumnDef> {
        vec![
            ColumnDef::new(0, FieldKind::LongLong),
            ColumnDef::new(1, FieldKind::LongLong),
        ]
    }
}
impl Display for SortCase {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "(rows:{}, orderBy:[{}], ndvs: [{}])",
            self.Rows,
            self.OrderByIdx
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(" "),
            self.Ndvs
                .iter()
                .map(i32::to_string)
                .collect::<Vec<_>>()
                .join(" ")
        )
    }
}
/// 构造默认 Sort 用例：30 万行，按列 0、1 排序，不限制内存。
pub fn DefaultSortTestCase(mut ctx: SessionContext) -> SortCase {
    resetChunkSizes(&mut ctx);
    ctx.vars.statement_memory_tracker = MemoryTracker {
        limit: -1,
        attached: false,
    };
    SortCase {
        Ctx: ctx,
        FileNamePrefixForTest: String::new(),
        Rows: 300_000,
        OrderByIdx: vec![0, 1],
        Ndvs: vec![0, 0],
    }
}
/// 构造带内存上限的 Sort 用例，便于测试 spill（内存不足时落盘）路径。
pub fn SortTestCaseWithMemoryLimit(mut ctx: SessionContext, bytes_limit: i64) -> SortCase {
    resetChunkSizes(&mut ctx);
    // 会话与语句级内存跟踪均设为同一上限，并标记 statement tracker 已挂接。
    ctx.vars.memory_tracker = MemoryTracker {
        limit: bytes_limit,
        attached: false,
    };
    ctx.vars.statement_memory_tracker = MemoryTracker {
        limit: bytes_limit,
        attached: true,
    };
    SortCase {
        Ctx: ctx,
        FileNamePrefixForTest: String::new(),
        Rows: 300_000,
        OrderByIdx: vec![0, 1],
        Ndvs: vec![0, 0],
    }
}
