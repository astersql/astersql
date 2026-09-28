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

// 窗口函数（Window）算子测试用例定义。
//
// 窗口函数在 PARTITION BY / ORDER BY 划定的窗口内为每行计算结果（如
// `row_number`、`rank`、聚合型窗口）；Frame 描述窗口起始/结束边界。

use crate::testutil::{ColumnDef, FieldKind, SessionContext, resetChunkSizes};
use std::fmt::{Display, Formatter};

/// 窗口帧边界：相对当前行的起止偏移（ROWS/RANGE 语义由执行器解释）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WindowFrame {
    /// 帧起始边界（相对当前行）。
    pub start: i64,
    /// 帧结束边界（相对当前行）。
    pub end: i64,
}
/// 窗口算子单次测试的参数集合。
pub struct WindowTestCase {
    /// 会话上下文。
    pub Ctx: SessionContext,
    /// 可选窗口帧；`None` 表示使用默认帧。
    pub Frame: Option<WindowFrame>,
    /// 窗口函数名（如 `row_number`）。
    pub WindowFunc: String,
    /// 小规模原始字符串数据（用于 VarString 列填充）。
    pub RawDataSmall: String,
    /// 输入列定义。
    pub Columns: Vec<ColumnDef>,
    /// 同时计算的窗口函数个数。
    pub NumFunc: usize,
    /// 分区键 NDV（不同分区个数）。
    pub Ndv: usize,
    /// 输入总行数。
    pub Rows: usize,
    /// 并行度。
    pub Concurrency: usize,
    /// 是否启用流水线执行（pipelined）；非 0 表示开启相关路径。
    pub Pipelined: i32,
    /// 数据源是否已按分区/排序键有序。
    pub DataSourceSorted: bool,
}
impl Display for WindowTestCase {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "(func:{}, aggColType:{}, numFunc:{}, ndv:{}, rows:{}, sorted:{}, concurrency:{}, pipelined:{})",
            self.WindowFunc,
            self.Columns[0].kind,
            self.NumFunc,
            self.Ndv,
            self.Rows,
            self.DataSourceSorted,
            self.Concurrency,
            self.Pipelined
        )
    }
}
/// 构造默认窗口用例：`row_number`、千万行、已排序、四列（Double/LongLong/VarString/LongLong）。
pub fn DefaultWindowTestCase(mut ctx: SessionContext) -> WindowTestCase {
    resetChunkSizes(&mut ctx);
    WindowTestCase {
        Ctx: ctx,
        Frame: None,
        WindowFunc: "row_number".into(),
        RawDataSmall: "x".repeat(16),
        NumFunc: 1,
        Ndv: 1000,
        Rows: 10_000_000,
        Concurrency: 1,
        Pipelined: 0,
        DataSourceSorted: true,
        Columns: vec![
            ColumnDef::new(0, FieldKind::Double),
            ColumnDef::new(1, FieldKind::LongLong),
            ColumnDef::new(2, FieldKind::VarString),
            ColumnDef::new(3, FieldKind::LongLong),
        ],
    }
}
