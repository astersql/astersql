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

// 聚合（Aggregation）算子性能测试用例定义。
//
// 聚合算子按分组键（GROUP BY）汇总行，支持 SUM/COUNT 等聚合函数，
// 以及 DISTINCT 去重；本模块描述用例参数与默认列布局。

use crate::testutil::{ColumnDef, FieldKind, SessionContext, resetChunkSizes};
use std::fmt::{Display, Formatter};

/// 聚合算子单次基准/功能测试的参数集合。
pub struct AggTestCase {
    /// 会话上下文（Chunk 大小等会话变量）。
    pub Ctx: SessionContext,
    /// 执行器类型标识字符串（如 hash agg / stream agg）。
    pub ExecType: String,
    /// 聚合函数名（如 `sum`）。
    pub AggFunc: String,
    /// GROUP BY 列的 NDV（Number of Distinct Values，不同值个数）。
    pub GroupByNDV: usize,
    /// 输入总行数。
    pub Rows: usize,
    /// 并行度（并发 worker 数）。
    pub Concurrency: usize,
    /// 数据源是否已按分组键有序（影响是否可走 stream agg）。
    pub DataSourceSorted: bool,
    /// 是否带 DISTINCT（对聚合参数去重后再汇总）。
    pub HasDistinct: bool,
}
impl AggTestCase {
    /// 返回用例默认列定义：第 0 列 Double（聚合输入），第 1 列 LongLong（分组键）。
    pub fn Columns(&self) -> Vec<ColumnDef> {
        vec![
            ColumnDef::new(0, FieldKind::Double),
            ColumnDef::new(1, FieldKind::LongLong),
        ]
    }
}
impl Display for AggTestCase {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "(execType:{}, aggFunc:{}, ndv:{}, hasDistinct:{}, rows:{}, concurrency:{}, sorted:{})",
            self.ExecType,
            self.AggFunc,
            self.GroupByNDV,
            self.HasDistinct,
            self.Rows,
            self.Concurrency,
            self.DataSourceSorted
        )
    }
}
/// 构造带默认参数的聚合测试用例：重置 Chunk 大小，默认 `sum`、千万行、并发 4、已排序。
pub fn DefaultAggTestCase(mut ctx: SessionContext, exec: String) -> AggTestCase {
    resetChunkSizes(&mut ctx);
    AggTestCase {
        Ctx: ctx,
        ExecType: exec,
        AggFunc: "sum".into(),
        GroupByNDV: 1000,
        HasDistinct: false,
        Rows: 10_000_000,
        Concurrency: 4,
        DataSourceSorted: true,
    }
}
