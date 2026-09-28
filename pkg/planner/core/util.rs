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

// 规划器核心工具：只读判定、聚合/窗口函数抽取、字符串拼接与脏表检测。
//
// 只读语句不修改用户数据，影响事务与副本路由；聚合函数（SUM/COUNT 等）与
// 窗口函数（ROW_NUMBER 等）需从 AST 抽出供优化。脏表指本会话已修改但未提交
// 到存储引擎可见版本的表。

use crate::{CIString, PlannerContext, SessionVars};
use std::collections::BTreeSet;

/// 简化的 AST 节点枚举，用于只读判定与函数抽取（非完整解析器 AST）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AstNode {
    /// SELECT 投影/子树列表。
    Select(Vec<AstNode>),
    /// EXPLAIN 包裹的语句。
    Explain(Box<AstNode>),
    /// DO 语句表达式列表。
    Do(Vec<AstNode>),
    /// SHOW 语句（视为只读）。
    Show,
    /// SET 语句；`global` 为 true 表示修改全局变量。
    Set { global: bool },
    /// INSERT；`replace` 表示 REPLACE INTO，`table_id` 为目标表。
    Insert { table_id: i64, replace: bool },
    /// UPDATE 目标表。
    Update { table_id: i64 },
    /// DELETE 目标表。
    Delete { table_id: i64 },
    /// 聚合函数调用。
    AggregateFunc { name: String, args: Vec<AstNode> },
    /// 窗口函数调用。
    WindowFunc { name: String, args: Vec<AstNode> },
    /// 子查询树（抽取时不深入其内部）。
    Subquery(Vec<AstNode>),
    /// 字面量等叶子值。
    Value(String),
    /// 其它节点：显式只读标志与子节点。
    Other {
        read_only: bool,
        children: Vec<AstNode>,
    },
}

/// 判断语句树是否只读（默认检查全局 SET）。
pub fn IsReadOnly(node: &AstNode, vars: &SessionVars) -> bool {
    IsReadOnlyInternal(node, vars, true)
}

/// 递归判定只读性。
///
/// `check_global_vars` 为 false 时忽略 SET GLOBAL 的写语义；DML 始终为写语句。
pub fn IsReadOnlyInternal(node: &AstNode, vars: &SessionVars, check_global_vars: bool) -> bool {
    match node {
        AstNode::Select(children) | AstNode::Do(children) => children
            .iter()
            .all(|child| IsReadOnlyInternal(child, vars, check_global_vars)),
        AstNode::Explain(statement) => IsReadOnlyInternal(statement, vars, check_global_vars),
        AstNode::Show => true,
        AstNode::Set { global } => !check_global_vars || !global,
        AstNode::Insert { .. } | AstNode::Update { .. } | AstNode::Delete { .. } => false,
        AstNode::AggregateFunc { args, .. }
        | AstNode::WindowFunc { args, .. }
        | AstNode::Subquery(args) => args
            .iter()
            .all(|child| IsReadOnlyInternal(child, vars, check_global_vars)),
        AstNode::Value(_) => true,
        AstNode::Other {
            read_only,
            children,
        } => {
            *read_only
                && children
                    .iter()
                    .all(|child| IsReadOnlyInternal(child, vars, check_global_vars))
        }
    }
}

/// 遍历 AST 收集聚合函数节点；遇到子查询则停止下钻。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AggregateFuncExtractor {
    /// 已收集的聚合函数节点。
    pub AggFuncs: Vec<AstNode>,
}
impl AggregateFuncExtractor {
    /// 进入节点前：子查询返回 false，阻止深入。
    pub fn Enter(&self, node: &AstNode) -> bool {
        !matches!(node, AstNode::Subquery(_))
    }
    /// 离开节点时若为聚合函数则收集。
    pub fn Leave(&mut self, node: &AstNode) {
        if let AstNode::AggregateFunc { .. } = node {
            self.AggFuncs.push(node.clone());
        }
    }
    /// 先序 Enter、递归子节点、后序 Leave。
    pub fn Extract(&mut self, node: &AstNode) {
        if !self.Enter(node) {
            return;
        }
        for child in children(node) {
            self.Extract(child);
        }
        self.Leave(node);
    }
}

/// 遍历 AST 收集窗口函数节点；遇到子查询则停止下钻。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WindowFuncExtractor {
    /// 已收集的窗口函数节点。
    pub WindowFuncs: Vec<AstNode>,
}
impl WindowFuncExtractor {
    /// 进入节点前：子查询返回 false，阻止深入。
    pub fn Enter(&self, node: &AstNode) -> bool {
        !matches!(node, AstNode::Subquery(_))
    }
    /// 离开节点时若为窗口函数则收集。
    pub fn Leave(&mut self, node: &AstNode) {
        if let AstNode::WindowFunc { .. } = node {
            self.WindowFuncs.push(node.clone());
        }
    }
    /// 先序 Enter、递归子节点、后序 Leave。
    pub fn Extract(&mut self, node: &AstNode) {
        if !self.Enter(node) {
            return;
        }
        for child in children(node) {
            self.Extract(child);
        }
        self.Leave(node);
    }
}

/// 返回节点的可递归子节点切片。
fn children(node: &AstNode) -> &[AstNode] {
    match node {
        AstNode::Select(nodes) | AstNode::Do(nodes) | AstNode::Subquery(nodes) => nodes,
        AstNode::AggregateFunc { args, .. } | AstNode::WindowFunc { args, .. } => args,
        AstNode::Other { children, .. } => children,
        _ => &[],
    }
}

/// 将有序字符串集合拼接为逗号分隔串。
pub fn extractStringFromStringSet(set: &BTreeSet<String>) -> String {
    set.iter()
        .map(|value| format!("\"{value}\""))
        .collect::<Vec<_>>()
        .join(",")
}
/// 就地排序后将字符串切片拼接为逗号分隔串。
pub fn extractStringFromStringSlice(slice: &mut [String]) -> String {
    slice.sort();
    slice.join(",")
}
/// 将 u64 切片转为逗号分隔十进制串。
pub fn extractStringFromUint64Slice(slice: &[u64]) -> String {
    let mut values = slice.iter().map(u64::to_string).collect::<Vec<_>>();
    values.sort();
    values.join(",")
}
/// 将 bool 切片转为逗号分隔 `"true"/"false"` 串。
pub fn extractStringFromBoolSlice(slice: &[bool]) -> String {
    let mut values = slice.iter().map(bool::to_string).collect::<Vec<_>>();
    values.sort();
    values.join(",")
}

/// 表元信息片段：逻辑表 ID、临时表属性与物理分区 ID。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableInfo {
    /// 表标识。
    pub id: i64,
    /// 是否为临时表；保留表元信息属性，脏内容判断由上下文中的 ID 决定。
    pub temp_table: bool,
    /// 分区表的物理分区 ID；空切片表示非分区表。
    pub partition_ids: Vec<i64>,
}
/// 非分区表检查表 ID；分区表检查每个物理分区 ID。
pub fn tableHasDirtyContent(ctx: &PlannerContext, table: &TableInfo) -> bool {
    if table.partition_ids.is_empty() {
        return ctx.vars.dirty_tables.contains(&table.id);
    }
    table
        .partition_ids
        .iter()
        .any(|partition_id| ctx.vars.dirty_tables.contains(partition_id))
}
/// 返回库名的规范小写形式。
pub fn getLowerDB(database: CIString, _vars: &SessionVars) -> String {
    database.L
}
