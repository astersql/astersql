// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 规划器杂项工具：递归切片扁平化、表达式/列克隆、查询时间范围、
// 排序前缀、表别名提取、下推上下文以及按隔离读引擎过滤访问路径。
//
// 「隔离读（isolation read）」指会话变量指定的可读存储引擎集合
// （如 TiKV / TiFlash）；过滤后若无可用路径则报错。

use expression::Expression as _;

use crate::{AccessPath, HandleCols};

/// Rust's typed replacement for Go's reflection-based arbitrary-dimensional slice.
#[derive(Clone, Debug, PartialEq, Eq)]
/// Rust 侧替代 Go 反射任意维切片：叶子为 Values，中间为嵌套 Slices。
pub enum RecursiveSlice<E> {
    /// 一维元素向量。
    Values(Vec<E>),
    /// 嵌套的递归切片。
    Slices(Vec<RecursiveSlice<E>>),
}

/// 深度优先扁平化 [`RecursiveSlice`] 的迭代器，产出全局下标与元素引用。
pub struct RecursiveFlattenIter<'a, E> {
    /// 待展开的嵌套切片迭代器栈。
    stack: Vec<std::slice::Iter<'a, RecursiveSlice<E>>>,
    /// 当前叶子 Values 的迭代器。
    values: Option<std::slice::Iter<'a, E>>,
    /// 已产出元素的全局下标（下一值）。
    index: usize,
}

/// 先耗尽当前 Values，再从栈中弹出下一层 Slices/Values。
impl<'a, E> Iterator for RecursiveFlattenIter<'a, E> {
    type Item = (usize, &'a E);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(values) = &mut self.values {
                if let Some(value) = values.next() {
                    let index = self.index;
                    self.index += 1;
                    return Some((index, value));
                }
                self.values = None;
            }
            let current = self.stack.last_mut()?;
            match current.next() {
                Some(RecursiveSlice::Values(values)) => self.values = Some(values.iter()),
                Some(RecursiveSlice::Slices(slices)) => self.stack.push(slices.iter()),
                None => {
                    self.stack.pop();
                }
            }
        }
    }
}

/// 构造从顶层切片开始的扁平化迭代器。
pub fn SliceRecursiveFlattenIter<E>(slice: &[RecursiveSlice<E>]) -> RecursiveFlattenIter<'_, E> {
    RecursiveFlattenIter {
        stack: vec![slice.iter()],
        values: None,
        index: 0,
    }
}

/// 克隆输出列名切片（NameSlice）。
pub fn CloneFieldNames(
    names: Option<&types::metadata::NameSlice>,
) -> Option<types::metadata::NameSlice> {
    names.map(|names| types::metadata::NameSlice(names.0.clone()))
}

/// 克隆表达式向量。
pub fn CloneExprs(exprs: Option<&[expression::ExprBox]>) -> Option<Vec<expression::ExprBox>> {
    exprs.map(|expressions| expressions.to_vec())
}

/// 克隆 UPDATE 赋值列表。
pub fn CloneAssignments(
    assignments: Option<&[expression::Assignment]>,
) -> Option<Vec<expression::Assignment>> {
    assignments.map(|items| items.iter().map(expression::Assignment::Clone).collect())
}

/// 克隆句柄列集合（调用各 HandleCols 的 CloneHandleCols）。
pub fn CloneHandleCols(
    handles: Option<&[Box<dyn HandleCols>]>,
) -> Option<Vec<Box<dyn HandleCols>>> {
    handles.map(|items| {
        items
            .iter()
            .map(|handle| handle.CloneHandleCols())
            .collect()
    })
}

/// 克隆可选列向量。
pub fn CloneCols(
    columns: Option<&[Option<expression::Column>]>,
) -> Option<Vec<Option<expression::Column>>> {
    columns.map(<[Option<expression::Column>]>::to_vec)
}

/// 克隆 Datum 一维向量。
pub fn CloneDatums(
    datums: Option<&[expression::types::Datum]>,
) -> Option<Vec<expression::types::Datum>> {
    datums.map(<[expression::types::Datum]>::to_vec)
}

/// 克隆 Datum 二维向量。
pub fn CloneDatum2D(
    datums: Option<&[Vec<expression::types::Datum>]>,
) -> Option<Vec<Vec<expression::types::Datum>>> {
    datums.map(<[Vec<expression::types::Datum>]>::to_vec)
}

/// 克隆 KV Handle 列表（Copy）。
pub fn CloneHandles(handles: Option<&[Box<dyn kv::Handle>]>) -> Option<Vec<Box<dyn kv::Handle>>> {
    handles.map(|items| items.iter().map(|handle| handle.Copy()).collect())
}

/// 指标/诊断查询的时间闭区间。
pub struct QueryTimeRange {
    /// 区间起点（含）。
    pub From: chrono::DateTime<chrono::FixedOffset>,
    /// 区间终点（含）。
    pub To: chrono::DateTime<chrono::FixedOffset>,
}

/// 生成 SQL WHERE 时间条件与内存估算。
impl QueryTimeRange {
    /// 格式化为 `where time>='...' and time<='...'`。
    pub fn Condition(&self) -> String {
        format!(
            "where time>='{}' and time<='{}'",
            self.From.format("%Y-%m-%d %H:%M:%S%.3f"),
            self.To.format("%Y-%m-%d %H:%M:%S%.3f")
        )
    }

    /// 结构体本身大小（不含堆分配）。
    pub fn MemoryUsage(&self) -> i64 {
        std::mem::size_of::<QueryTimeRange>() as i64
    }
}

/// 指标表时间字符串格式（Go 参考时间布局）。
pub const MetricTableTimeFormat: &str = "2006-01-02 15:04:05.999";

/// 将 i32 按大端 uint32 追加到字节缓冲。
pub fn EncodeIntAsUint32(mut result: Vec<u8>, value: i32) -> Vec<u8> {
    result.extend_from_slice(&(value as u32).to_be_bytes());
    result
}

/// 计算排序列在 all_columns schema 中的最长连续前缀下标。
/// 遇首个不在 schema 中的列即停止（排序键必须是表列前缀才能下推）。
pub fn GetMaxSortPrefix(
    sort_columns: &[expression::Column],
    all_columns: &[expression::Column],
) -> Vec<usize> {
    let schema = expression::NewSchema(all_columns.to_vec());
    let mut offsets = Vec::with_capacity(sort_columns.len());
    for column in sort_columns {
        let Some(offset) = schema.ColumnIndex(column) else {
            break;
        };
        offsets.push(offset);
    }
    offsets
}

/// 从计划输出名提取单一表别名，供 hint 匹配；多表或无名则返回 None。
pub fn ExtractTableAlias(
    plan: &dyn plan_base::Plan,
    parent_offset: i32,
) -> Option<hint::HintedTable> {
    let names = plan.output_names();
    let first_name = names
        .0
        .iter()
        .flatten()
        .find(|name| !name.TblName.L.is_empty())?;
    for name in names.0.iter().flatten() {
        if name.TblName.L.is_empty() {
            if !name.DBName.L.is_empty() {
                return None;
            }
            continue;
        }
        if name.TblName.L != first_name.TblName.L
            || (!name.DBName.L.is_empty()
                && !first_name.DBName.L.is_empty()
                && name.DBName.L != first_name.DBName.L)
        {
            return None;
        }
    }

    // 若当前块在 PlannerSelectBlockAsName 中有显式表名，则回退到父块 offset。
    let mut query_block_offset = plan.query_block_offset();
    let session = plan.s_ctx().GetSessionVars();
    if query_block_offset != parent_offset
        && query_block_offset >= 0
        && session
            .PlannerSelectBlockAsName
            .Load()
            .is_some_and(|names| {
                names
                    .get(query_block_offset as usize)
                    .is_some_and(|table| !table.TableName.L.is_empty())
            })
    {
        query_block_offset = parent_offset;
    }
    let database = if first_name.DBName.L.is_empty() {
        parser_ast::NewCIStr(&session.CurrentDB())
    } else {
        first_name.DBName.clone()
    };
    Some(hint::HintedTable {
        DBName: database,
        TblName: first_name.TblName.clone(),
        SelectOffset: query_block_offset,
        ..Default::default()
    })
}

/// 从 PlanContext 取出表达式下推上下文。
pub fn GetPushDownCtx(context: &dyn plan_base::PlanContext) -> expression::PushDownContext {
    GetPushDownCtxFromBuildPBContext(context.GetBuildPBCtx())
}

/// 由 BuildPBContext 组装 PushDownContext（表达式下推到存储层时用）。
pub fn GetPushDownCtxFromBuildPBContext(
    context: &plan_base::BuildPBContext,
) -> expression::PushDownContext {
    expression::NewPushDownContext(
        context.GetExprCtx(),
        context.GetClient(),
        context.InExplainStmt,
        context.WarnHandler.clone(),
        context.ExtraWarnghandler.clone(),
        context.GroupConcatMaxLen,
    )
}

/// 是否需要检查 TiFlash 下推：计划含 TiFlash 且隔离读引擎包含 TiFlash。
pub fn ShouldCheckTiFlashPushDown(context: &dyn plan_base::PlanContext, has_tiflash: bool) -> bool {
    has_tiflash
        && context
            .GetSessionVars()
            .GetIsolationReadEngines()
            .contains(&kv::StoreType::TiFlash)
}

/// 按会话隔离读引擎过滤 AccessPath；系统库不过滤。
/// 若过滤后为空则报错；若未包含 TiFlash 则可能对 MPP 强制给出警告。
pub fn FilterPathByIsolationRead(
    context: &dyn plan_base::PlanContext,
    mut paths: Vec<AccessPath>,
    table_name: parser_ast::CIStr,
    database_name: parser_ast::CIStr,
) -> Result<Vec<AccessPath>, expression::Error> {
    if metadef::IsSystemRelatedDB(&database_name.L) {
        return Ok(paths);
    }
    let isolation_read_engines = context.GetSessionVars().GetIsolationReadEngines();
    let mut available_engines = Vec::new();
    for path in paths.iter().rev() {
        if !available_engines.contains(&path.StoreType) {
            available_engines.push(path.StoreType);
        }
    }
    // 保留隔离读允许的引擎路径，TiDB 引擎路径始终保留。
    paths.retain(|path| {
        isolation_read_engines.contains(&path.StoreType) || path.StoreType == kv::StoreType::TiDB
    });

    let engine_values = context
        .GetSessionVars()
        .GetSystemVar(vardef::TiDBIsolationReadEngines)
        .unwrap_or_default();
    // 无可用路径：拼接可用引擎与 TiFlash 副本/只读提示后报错。
    let no_path_error = if paths.is_empty() {
        let available = available_engines
            .iter()
            .map(|engine| engine.Name())
            .collect::<Vec<_>>()
            .join(", ");
        let mut help = String::new();
        if engine_values.contains("tiflash") {
            help.push_str(". Please check tiflash replica");
            if context
                .GetSessionVars()
                .StmtCtx
                .TiFlashEngineRemovedDueToStrictSQLMode
            {
                help.push_str(" or check if the query is not readonly and sql mode is strict");
            }
        }
        Some(expression::errors::New(format!(
            "No access path for table '{}' is found with '{}' = '{}', valid values can be '{}'{help}.",
            table_name.O,
            vardef::TiDBIsolationReadEngines,
            engine_values,
            available,
        )))
    } else {
        None
    };

    if !isolation_read_engines.contains(&kv::StoreType::TiFlash) {
        let warning = if context
            .GetSessionVars()
            .StmtCtx
            .TiFlashEngineRemovedDueToStrictSQLMode
        {
            "MPP mode may be blocked because the query is not readonly and sql mode is strict."
                .to_owned()
        } else {
            format!(
                "MPP mode may be blocked because '{}'(value: '{}') not match, need 'tiflash'.",
                vardef::TiDBIsolationReadEngines,
                engine_values,
            )
        };
        context
            .GetSessionVars()
            .RaiseWarningWhenMPPEnforced(warning);
    }
    match no_path_error {
        Some(error) => Err(error),
        None => Ok(paths),
    }
}
