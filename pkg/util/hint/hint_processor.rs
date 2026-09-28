// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 优化器 Hint 处理器：收集、绑定、还原与 SQL Binding 完整性检查。
//
// 对应 Go `hintprocessor`：从 AST 收集表/索引 hint（HintsSet）、写回 AST（BindHint）、
// 将 hint 还原为文本、通过真实 parser 解析 binding SQL（ParseHintsSet），
// 并判断历史 binding 是否因多表 join / 子查询 / TiFlash 而不完整。

use std::collections::{HashMap, HashSet};

use crate::hint_query_block::{GenerateQBName, NewQBHintHandler, hintQBName, hintWarnHandler};
use crate::{ast, errors, isStmtHint};

/// INSERT 语句允许的优化器 hint 名称集合（当前仅 `memory_quota`）。
pub fn supportedHintNameForInsertStmt() -> HashSet<&'static str> {
    HashSet::from(["memory_quota"])
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 一份语句上收集到的表级与索引级 hint 集合。
///
/// `tableHints`/`indexHints` 按查询块或表出现顺序分层存放，便于后续绑定回 AST。
pub struct HintsSet {
    /// 各查询块的 TableOptimizerHint 列表（外层下标对应块顺序）。
    /// 各查询块的表优化器 hint（外层向量按块顺序）。
    pub tableHints: Vec<Vec<ast::TableOptimizerHint>>,
    /// 各表源上的 IndexHint 列表（遍历顺序与 collect 一致）。
    /// 各表源上的索引 hint（与遍历顺序对应）。
    pub indexHints: Vec<Vec<ast::IndexHint>>,
}

impl HintsSet {
    /// 取“语句级”hint：首块全部 + 后续块中仅 `isStmtHint` 为真的项。
    /// 取语句级 hint：首块全部保留，后续块仅保留语句级 hint。
    pub fn GetStmtHints(&self) -> Vec<ast::TableOptimizerHint> {
        let mut result = Vec::new();
        // 第一个查询块的 hint 全部视为语句级（含非 stmt hint）。
        if let Some(first) = self.tableHints.first() {
            result.extend(first.iter().cloned());
        }
        // 深层查询块只抽取 `isStmtHint` 判定为语句级的 hint。
        for blockHints in self.tableHints.iter().skip(1) {
            result.extend(blockHints.iter().filter(|hint| isStmtHint(hint)).cloned());
        }
        result
    }

    /// 是否包含指定原始大小写名（比较 `HintName.O`）的表 hint。
    /// 是否包含指定原始名（`HintName.O`）的表 hint。
    pub fn ContainTableHint(&self, name: &str) -> bool {
        self.tableHints
            .iter()
            .flatten()
            .any(|hint| hint.HintName.O == name)
    }

    /// 将全部表/索引 hint 还原为逗号分隔文本。
    /// 将集合中全部 hint 还原为逗号分隔的文本。
    pub fn Restore(&self) -> Result<String, errors::Error> {
        let mut restored = Vec::new();
        for hint in self.tableHints.iter().flatten() {
            restored.push(RestoreTableOptimizerHint(hint));
        }
        for hint in self.indexHints.iter().flatten() {
            restored.push(RestoreIndexHint(hint)?);
        }
        Ok(restored.join(", "))
    }
}

/// 从 SELECT/UPDATE/DELETE/INSERT 节点读取 `TableHints` 字段。
fn tableHints(node: &dyn ast::Node) -> Vec<ast::TableOptimizerHint> {
    if let Some(stmt) = node.as_any().downcast_ref::<ast::SelectStmt>() {
        return stmt.TableHints.clone();
    }
    if let Some(stmt) = node.as_any().downcast_ref::<ast::UpdateStmt>() {
        return stmt.TableHints.clone();
    }
    if let Some(stmt) = node.as_any().downcast_ref::<ast::DeleteStmt>() {
        return stmt.TableHints.clone();
    }
    // INSERT ... SELECT：先告警重复 hint，再合并 INSERT 自身与 SELECT 语句级 hint。
    if let Some(stmt) = node.as_any().downcast_ref::<ast::InsertStmt>() {
        return stmt.TableHints.clone();
    }
    Vec::new()
}

/// 将表 hint 写回 SELECT/UPDATE/DELETE 节点；其它节点原样返回。
pub fn setTableHints4StmtNode(
    node: Box<dyn ast::Node>,
    hints: Vec<ast::TableOptimizerHint>,
) -> Box<dyn ast::Node> {
    if node.as_any().is::<ast::SelectStmt>() {
        let mut stmt = node.into_any().downcast::<ast::SelectStmt>().unwrap();
        stmt.TableHints = hints;
        return stmt;
    }
    if node.as_any().is::<ast::UpdateStmt>() {
        let mut stmt = node.into_any().downcast::<ast::UpdateStmt>().unwrap();
        stmt.TableHints = hints;
        return stmt;
    }
    if node.as_any().is::<ast::DeleteStmt>() {
        let mut stmt = node.into_any().downcast::<ast::DeleteStmt>().unwrap();
        stmt.TableHints = hints;
        return stmt;
    }
    node
}

/// 递归抽取节点上的表 hint；INSERT 会合并 SELECT 侧语句级 hint 并检查重复。
fn extractTableHints(
    node: &dyn ast::Node,
    warnHandler: &mut Option<&mut dyn hintWarnHandler>,
) -> Vec<ast::TableOptimizerHint> {
    if node.as_any().is::<ast::SelectStmt>()
        || node.as_any().is::<ast::UpdateStmt>()
        || node.as_any().is::<ast::DeleteStmt>()
    {
        return tableHints(node);
    }
    if let Some(stmt) = node.as_any().downcast_ref::<ast::InsertStmt>() {
        // INSERT 与内嵌 SELECT 同名 hint 冲突时告警。
        warnInsertHintDuplicated(stmt, warnHandler);
        let mut result = stmt.TableHints.clone();
        if let Some(select) = stmt.Select.as_deref() {
            result.extend(
                extractTableHints(select, warnHandler)
                    .into_iter()
                    .filter(isStmtHint),
            );
        }
        return result;
    }
    // UNION/INTERSECT 等集合运算：汇总各 SELECT 分支的 hint。
    if let Some(stmt) = node.as_any().downcast_ref::<ast::SetOprStmt>() {
        let mut result = Vec::new();
        for select in &stmt.select_list.selects {
            result.extend(extractTableHints(select.as_ref(), warnHandler));
        }
        return result;
    }
    Vec::new()
}

/// 对外入口：从语句节点抽取表优化器 hint，可选告警回调。
pub fn ExtractTableHintsFromStmtNode(
    node: &dyn ast::Node,
    warnHandler: Option<&mut dyn hintWarnHandler>,
) -> Vec<ast::TableOptimizerHint> {
    let mut warnHandler = warnHandler;
    extractTableHints(node, &mut warnHandler)
}

/// 在 hint 切片中按小写名（`HintName.L`）查找。
fn containTableHint(hints: &[ast::TableOptimizerHint], name: &str) -> bool {
    hints.iter().any(|hint| hint.HintName.L == name)
}

/// 判断语句（含 INSERT 子查询、集合运算分支）是否包含指定表 hint。
pub fn ContainTableHintInStmtNode(node: &dyn ast::Node, name: &str) -> bool {
    if node.as_any().is::<ast::SelectStmt>()
        || node.as_any().is::<ast::UpdateStmt>()
        || node.as_any().is::<ast::DeleteStmt>()
    {
        return containTableHint(&tableHints(node), name);
    }
    if let Some(stmt) = node.as_any().downcast_ref::<ast::InsertStmt>() {
        return containTableHint(&stmt.TableHints, name)
            || stmt
                .Select
                .as_deref()
                .is_some_and(|select| ContainTableHintInStmtNode(select, name));
    }
    if let Some(stmt) = node.as_any().downcast_ref::<ast::SetOprStmt>() {
        return stmt
            .select_list
            .selects
            .iter()
            .any(|select| ContainTableHintInStmtNode(select.as_ref(), name));
    }
    false
}

/// INSERT 与其 SELECT 侧出现同名受支持 hint 时，通过 warnHandler 发出冲突警告。
fn warnInsertHintDuplicated(
    stmt: &ast::InsertStmt,
    warnHandler: &mut Option<&mut dyn hintWarnHandler>,
) {
    let supported = supportedHintNameForInsertStmt();
    let Some(insertHint) = stmt
        .TableHints
        .iter()
        .find(|hint| supported.contains(hint.HintName.L.as_str()))
    else {
        return;
    };
    let Some(select) = stmt.Select.as_deref() else {
        return;
    };
    // 提取 SELECT 侧 hint 时静默，避免重复告警。
    let mut noWarnings = None;
    let Some(duplicated) = extractTableHints(select, &mut noWarnings)
        .into_iter()
        .find(|hint| hint.HintName.L == insertHint.HintName.L)
    else {
        return;
    };
    let Some(handler) = warnHandler.as_deref_mut() else {
        return;
    };
    let text = format!("{}(`{:?}`)", duplicated.HintName.O, duplicated.HintData);
    let error = plannererrors::planner_terror::ErrWarnConflictingHint
        .FastGenByArgs(&[plannererrors::errors::ErrorArg::String(text)]);
    handler.SetHintWarningFromError(&error);
}

/// 若节点为 INSERT，检查并报告与 SELECT 侧重复的受支持 hint。
pub fn checkInsertStmtHintDuplicated(
    node: &dyn ast::Node,
    warnHandler: Option<&mut dyn hintWarnHandler>,
) {
    let Some(stmt) = node.as_any().downcast_ref::<ast::InsertStmt>() else {
        return;
    };
    let mut warnHandler = warnHandler;
    warnInsertHintDuplicated(stmt, &mut warnHandler);
}

/// 还原表优化器 hint 列表为去重后的逗号分隔文本。
pub fn RestoreOptimizerHints(hints: Vec<ast::TableOptimizerHint>) -> String {
    let mut seen = HashMap::with_capacity(hints.len());
    let mut restored = Vec::with_capacity(hints.len());
    // 用 HashMap 保序去重：相同还原文本只保留首次出现。
    for hint in &hints {
        let value = RestoreTableOptimizerHint(hint);
        if seen.insert(value.clone(), ()).is_none() {
            restored.push(value);
        }
    }
    restored.join(", ")
}

/// 将标识符包成反引号，内部反引号加倍转义。
fn quoteName(name: &str) -> String {
    format!("`{}`", name.replace('`', "``"))
}

/// 将字符串写成单引号字面量，转义控制字符与引号。
fn quoteString(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 2);
    output.push('\'');
    for ch in value.chars() {
        match ch {
            '\0' => output.push_str("\\0"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\\' => output.push_str("\\\\"),
            '\'' => output.push_str("''"),
            _ => output.push(ch),
        }
    }
    output.push('\'');
    output
}

/// 还原 hint 中的表引用：`db.table@qb PARTITION(...)`。
fn restoreHintTable(table: &ast::HintTable) -> String {
    let mut output = String::new();
    if !table.DBName.L.is_empty() {
        output.push_str(&quoteName(&table.DBName.O));
        output.push('.');
    }
    output.push_str(&quoteName(&table.TableName.O));
    if !table.QBName.L.is_empty() {
        output.push('@');
        output.push_str(&quoteName(&table.QBName.O));
    }
    if !table.PartitionList.is_empty() {
        output.push_str(" PARTITION(");
        output.push_str(
            &table
                .PartitionList
                .iter()
                .map(|partition| quoteName(&partition.O))
                .collect::<Vec<_>>()
                .join(", "),
        );
        output.push(')');
    }
    output
}

/// 还原 `LEADING` hint 的嵌套表/列表参数。
fn restoreLeadingItems(items: &[ast::LeadingItem], needParen: bool) -> String {
    let mut output = String::new();
    if needParen {
        output.push('(');
    }
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            output.push_str(", ");
        }
        match item {
            ast::LeadingItem::Table(table) => output.push_str(&restoreHintTable(table)),
            ast::LeadingItem::List(list) => {
                output.push_str(&restoreLeadingItems(&list.Items, true));
            }
        }
    }
    if needParen {
        output.push(')');
    }
    output
}

/// 将单个表优化器 hint 还原为小写文本（如 `hash_join(`t`)`）。
pub fn RestoreTableOptimizerHint(hint: &ast::TableOptimizerHint) -> String {
    // 按 hint 小写名分支：无参、数值、表列表、索引列表等不同参数形态。
    let name = hint.HintName.L.as_str();
    let mut output = hint.HintName.O.to_uppercase();
    output.push('(');
    // 非 leading：先输出 @qb_name（qb_name hint 本身不加 @）。
    if name != "leading" && !hint.QBName.L.is_empty() {
        if name != hintQBName {
            output.push('@');
        }
        output.push_str(&quoteName(&hint.QBName.O));
    }
    if name == hintQBName && hint.Tables.is_empty() {
        output.push(')');
        return output.to_lowercase();
    }
    // 无参数类 hint：直接闭合括号。
    // 一批无参数的语句级/算子级 hint：直接闭合括号。
    if matches!(
        name,
        "mpp_1phase_agg"
            | "mpp_2phase_agg"
            | "hash_agg"
            | "stream_agg"
            | "agg_to_cop"
            | "read_consistent_replica"
            | "no_index_merge"
            | "ignore_plan_cache"
            | "use_plan_cache"
            | "limit_to_cop"
            | "straight_join"
            | "merge"
            | "no_decorrelate"
    ) {
        output.push(')');
        return output.to_lowercase();
    }
    if name != "leading" && !hint.QBName.L.is_empty() {
        output.push(' ');
    }

    // 按 hint 名分支输出参数（表列表、索引、数值、布尔等）。
    match name {
        "max_execution_time" => {
            if let ast::HintData::Unsigned(value) = &hint.HintData {
                output.push_str(&value.to_string());
            }
        }
        "resource_group" => match &hint.HintData {
            ast::HintData::Name(value) => output.push_str(&quoteName(value)),
            ast::HintData::CIStr(value) => output.push_str(&quoteName(&value.O)),
            _ => {}
        },
        "nth_plan" => {
            if let ast::HintData::Signed(value) = &hint.HintData {
                output.push_str(&value.to_string());
            }
        }
        "leading" => match &hint.HintData {
            ast::HintData::Leading(list) => {
                output.push_str(&restoreLeadingItems(&list.Items, false))
            }
            _ => output.push_str(
                &hint
                    .Tables
                    .iter()
                    .map(restoreHintTable)
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        },
        "tidb_hj"
        | "tidb_smj"
        | "tidb_inlj"
        | "hash_join"
        | "hash_join_build"
        | "hash_join_probe"
        | "merge_join"
        | "inl_join"
        | "broadcast_join"
        | "shuffle_join"
        | "inl_hash_join"
        | "inl_merge_join"
        | "no_hash_join"
        | "no_merge_join"
        | "no_index_join"
        | "no_index_hash_join"
        | "no_index_merge_join" => output.push_str(
            &hint
                .Tables
                .iter()
                .map(restoreHintTable)
                .collect::<Vec<_>>()
                .join(", "),
        ),
        "use_index"
        | "ignore_index"
        | "use_index_merge"
        | "force_index"
        | "order_index"
        | "no_order_index"
        | "index_lookup_pushdown"
        | "no_index_lookup_pushdown" => {
            if let Some(table) = hint.Tables.first() {
                output.push_str(&restoreHintTable(table));
                output.push(' ');
                output.push_str(
                    &hint
                        .Indexes
                        .iter()
                        .map(|index| quoteName(&index.O))
                        .collect::<Vec<_>>()
                        .join(", "),
                );
            }
        }
        "qb_name" => {
            if !hint.Tables.is_empty() {
                output.push_str(", ");
                output.push_str(
                    &hint
                        .Tables
                        .iter()
                        .map(restoreHintTable)
                        .collect::<Vec<_>>()
                        .join(". "),
                );
            }
        }
        "use_toja" | "use_cascades" => {
            if let ast::HintData::Boolean(value) = &hint.HintData {
                output.push_str(if *value { "TRUE" } else { "FALSE" });
            }
        }
        "query_type" => match &hint.HintData {
            ast::HintData::CIStr(value) => output.push_str(&value.O.to_uppercase()),
            ast::HintData::Name(value) => output.push_str(&value.to_uppercase()),
            _ => {}
        },
        "memory_quota" => {
            // HintData 存字节，还原为 MB。
            if let ast::HintData::Signed(value) = &hint.HintData {
                output.push_str(&format!("{} MB", value / 1024 / 1024));
            }
        }
        "read_from_storage" => {
            match &hint.HintData {
                ast::HintData::CIStr(value) => output.push_str(&value.O.to_uppercase()),
                ast::HintData::Name(value) => output.push_str(&value.to_uppercase()),
                _ => {}
            }
            if !hint.Tables.is_empty() {
                output.push('[');
                output.push_str(
                    &hint
                        .Tables
                        .iter()
                        .map(restoreHintTable)
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                output.push(']');
            }
        }
        "time_range" => {
            if let ast::HintData::TimeRange(range) = &hint.HintData {
                output.push_str(&quoteString(&range.From));
                output.push_str(", ");
                output.push_str(&quoteString(&range.To));
            }
        }
        "set_var" => {
            if let ast::HintData::SetVar(value) = &hint.HintData {
                output.push_str(&value.VarName);
                output.push_str(" = ");
                output.push_str(&quoteString(&value.Value));
            }
        }
        _ => {}
    }
    output.push(')');
    output.to_lowercase()
}

/// 还原索引 hint（USE/IGNORE/FORCE INDEX 及作用域）。
pub fn RestoreIndexHint(hint: &ast::IndexHint) -> Result<String, errors::Error> {
    let hintType = match hint.HintType {
        ast::IndexHintType::Use => "USE INDEX",
        ast::IndexHintType::Ignore => "IGNORE INDEX",
        ast::IndexHintType::Force => "FORCE INDEX",
        ast::IndexHintType::OrderIndex => "ORDER INDEX",
        ast::IndexHintType::NoOrderIndex => "NO ORDER INDEX",
    };
    let scope = match hint.HintScope {
        ast::IndexHintScope::Scan => "",
        ast::IndexHintScope::Join => " FOR JOIN",
        ast::IndexHintScope::OrderBy => " FOR ORDER BY",
        ast::IndexHintScope::GroupBy => " FOR GROUP BY",
    };
    Ok(format!(
        "{}{} ({})",
        hintType,
        scope,
        hint.IndexNames
            .iter()
            .map(|name| quoteName(&name.O))
            .collect::<Vec<_>>()
            .join(", ")
    )
    .to_lowercase())
}

#[derive(Default)]
/// AST 遍历状态：收集或回写 hint，并用计数器对齐表/索引位置。
struct hintProcessor {
    /// 正在收集或待绑定的 hint 集合。
    HintsSet: HintsSet,
    /// 为 true 时表示绑定模式（写回 AST）；收集模式为 false。
    bindHint2Ast: bool,
    /// 绑定模式下已消费的表 hint 块下标。
    tableCounter: usize,
    /// 绑定模式下已消费的索引 hint 块下标。
    indexCounter: usize,
    /// 当前嵌套查询块深度（>0 时才记录索引 hint）。
    blockCounter: usize,
}

struct collectExprQueries<'a> {
    processor: &'a mut hintProcessor,
}

impl ast::ExprNodeVisitor for collectExprQueries<'_> {
    fn Enter(&mut self, input: &ast::ExprNode) -> (ast::ExprNode, bool) {
        if let ast::ExprKind::Subquery { Query, .. } = &input.Kind {
            Query.with_node(|node| collectNode(node, self.processor));
            return (input.clone(), true);
        }
        (input.clone(), false)
    }

    fn Leave(&mut self, input: &ast::ExprNode) -> (ast::ExprNode, bool) {
        (input.clone(), true)
    }
}

fn collectExpr(expr: &ast::ExprNode, processor: &mut hintProcessor) {
    let _ = expr.Accept(&mut collectExprQueries { processor });
}

fn collectTableName(table: &ast::TableName, processor: &mut hintProcessor) {
    if processor.blockCounter > 0 {
        processor.HintsSet.indexHints.push(table.IndexHints.clone());
    }
}

fn collectWith(with: &ast::WithClause, processor: &mut hintProcessor) {
    for cte in &with.CTEs {
        collectNode(cte.Query.as_ref(), processor);
    }
}

fn collectByItems(items: &[ast::ByItem], processor: &mut hintProcessor) {
    for item in items {
        collectExpr(&item.Expr, processor);
    }
}

fn collectLimit(limit: &ast::Limit, processor: &mut hintProcessor) {
    if let Some(offset) = &limit.Offset {
        collectExpr(offset, processor);
    }
    if let Some(count) = &limit.Count {
        collectExpr(count, processor);
    }
}

fn collectFields(fields: &ast::FieldList, processor: &mut hintProcessor) {
    for field in &fields.Fields {
        if let Some(expr) = &field.Expr {
            collectExpr(expr, processor);
        }
    }
}

fn collectWindowSpec(spec: &ast::WindowSpec, processor: &mut hintProcessor) {
    collectByItems(&spec.PartitionBy, processor);
    collectByItems(&spec.OrderBy, processor);
    if let Some(frame) = &spec.Frame {
        if let Some(expr) = &frame.Extent.Start.Expr {
            collectExpr(expr, processor);
        }
        if let Some(expr) = &frame.Extent.End.Expr {
            collectExpr(expr, processor);
        }
    }
}

/// 收集 TableSource 上的索引 hint，并下钻子查询。
fn collectTableSource(source: &ast::TableSource, processor: &mut hintProcessor) {
    if let Some(query) = &source.QuerySource {
        query.with_node(|node| collectNode(node, processor));
    } else {
        collectTableName(&source.Source, processor);
        if let Some(sample) = &source.TableSample {
            if let Some(expr) = &sample.Expr {
                collectExpr(expr, processor);
            }
            if let Some(expr) = &sample.RepeatableSeed {
                collectExpr(expr, processor);
            }
        }
        if let Some(as_of) = &source.AsOf {
            collectExpr(&as_of.TsExpr, processor);
        }
    }
}

/// 按 ResultSet 变体分发到表源或 Join 收集逻辑。
fn collectResultSet(result: &ast::ResultSetNode, processor: &mut hintProcessor) {
    match result {
        ast::ResultSetNode::TableSource(source) => collectTableSource(source, processor),
        ast::ResultSetNode::Join(join) => collectJoin(join, processor),
    }
}

/// 递归收集 Join 左右子树中的 hint。
fn collectJoin(join: &ast::Join, processor: &mut hintProcessor) {
    if let Some(left) = join.Left.as_deref() {
        collectResultSet(left, processor);
    }
    if let Some(right) = join.Right.as_deref() {
        collectResultSet(right, processor);
    }
    if let Some(on) = &join.On {
        collectExpr(on, processor);
    }
}

/// 深度优先遍历语句 AST，按查询块推入 tableHints，并收集索引 hint。
fn collectNode(node: &dyn ast::Node, processor: &mut hintProcessor) {
    if let Some(stmt) = node.as_any().downcast_ref::<ast::SelectStmt>() {
        processor.HintsSet.tableHints.push(stmt.TableHints.clone());
        processor.blockCounter += 1;
        if let Some(with) = &stmt.With {
            collectWith(&with.borrow(), processor);
        }
        collectFields(&stmt.Fields, processor);
        if let Some(from) = &stmt.From {
            collectJoin(&from.TableRefs, processor);
        }
        if let Some(where_expr) = &stmt.Where {
            collectExpr(where_expr, processor);
        }
        collectByItems(&stmt.GroupBy, processor);
        if let Some(having) = &stmt.Having {
            collectExpr(having, processor);
        }
        for row in &stmt.Lists {
            for value in &row.Values {
                collectExpr(value, processor);
            }
        }
        for spec in &stmt.WindowSpecs {
            collectWindowSpec(spec, processor);
        }
        collectByItems(&stmt.OrderBy, processor);
        if let Some(limit) = &stmt.Limit {
            collectLimit(limit, processor);
        }
        if let Some(lock_info) = &stmt.lock_info {
            for table in &lock_info.Tables {
                collectTableName(table, processor);
            }
        }
        for child in &stmt.children {
            collectNode(child.as_ref(), processor);
        }
        processor.blockCounter -= 1;
        return;
    }
    if let Some(stmt) = node.as_any().downcast_ref::<ast::UpdateStmt>() {
        processor.HintsSet.tableHints.push(stmt.TableHints.clone());
        processor.blockCounter += 1;
        if let Some(from) = &stmt.TableRefs {
            collectJoin(&from.TableRefs, processor);
        }
        for assignment in &stmt.List {
            collectExpr(&assignment.Expr, processor);
        }
        if let Some(where_expr) = &stmt.Where {
            collectExpr(where_expr, processor);
        }
        collectByItems(&stmt.Order, processor);
        if let Some(limit) = &stmt.Limit {
            collectLimit(limit, processor);
        }
        for field in &stmt.Returning {
            if let Some(expr) = &field.Expr {
                collectExpr(expr, processor);
            }
        }
        processor.blockCounter -= 1;
        return;
    }
    if let Some(stmt) = node.as_any().downcast_ref::<ast::DeleteStmt>() {
        processor.HintsSet.tableHints.push(stmt.TableHints.clone());
        processor.blockCounter += 1;
        if let Some(from) = &stmt.TableRefs {
            collectJoin(&from.TableRefs, processor);
        }
        for table in &stmt.Tables {
            collectTableName(table, processor);
        }
        if let Some(where_expr) = &stmt.Where {
            collectExpr(where_expr, processor);
        }
        collectByItems(&stmt.Order, processor);
        if let Some(limit) = &stmt.Limit {
            collectLimit(limit, processor);
        }
        for field in &stmt.Returning {
            if let Some(expr) = &field.Expr {
                collectExpr(expr, processor);
            }
        }
        processor.blockCounter -= 1;
        return;
    }
    if let Some(stmt) = node.as_any().downcast_ref::<ast::InsertStmt>() {
        if let Some(select) = stmt.Select.as_deref() {
            collectNode(select, processor);
        }
        if let Some(table) = &stmt.Table {
            collectJoin(&table.TableRefs, processor);
        }
        for row in &stmt.Lists {
            for value in row {
                collectExpr(value, processor);
            }
        }
        for assignment in &stmt.OnDuplicate {
            collectExpr(&assignment.Expr, processor);
        }
        for field in &stmt.Returning {
            if let Some(expr) = &field.Expr {
                collectExpr(expr, processor);
            }
        }
        return;
    }
    if let Some(stmt) = node.as_any().downcast_ref::<ast::SetOprStmt>() {
        if let Some(with) = &stmt.With {
            collectWith(&with.borrow(), processor);
        }
        if let Some(with) = &stmt.select_list.With {
            collectWith(&with.borrow(), processor);
        }
        for select in &stmt.select_list.selects {
            collectNode(select.as_ref(), processor);
        }
        collectByItems(&stmt.select_list.OrderBy, processor);
        if let Some(limit) = &stmt.select_list.Limit {
            collectLimit(limit, processor);
        }
        collectByItems(&stmt.OrderBy, processor);
        if let Some(limit) = &stmt.Limit {
            collectLimit(limit, processor);
        }
        return;
    }
    if let Some(table) = node.as_any().downcast_ref::<ast::TableName>() {
        collectTableName(table, processor);
    }
}

/// 从语句 AST 收集完整 `HintsSet`（表 hint + 索引 hint）。
pub fn CollectHint(statement: &dyn ast::Node) -> HintsSet {
    let mut processor = hintProcessor::default();
    processor.HintsSet.tableHints = Vec::with_capacity(4);
    processor.HintsSet.indexHints = Vec::with_capacity(4);
    collectNode(statement, &mut processor);
    processor.HintsSet
}

struct bindExprQueries<'a> {
    processor: &'a mut hintProcessor,
}

impl ast::ExprNodeVisitor for bindExprQueries<'_> {
    fn Enter(&mut self, input: &ast::ExprNode) -> (ast::ExprNode, bool) {
        let mut output = input.clone();
        if let ast::ExprKind::Subquery {
            Query,
            MultiRows,
            Exists,
        } = &input.Kind
        {
            if let Some(node) = Query.take() {
                output.Kind = ast::ExprKind::Subquery {
                    Query: ast::NodeRef::new(bindNode(node, self.processor)),
                    MultiRows: *MultiRows,
                    Exists: *Exists,
                };
            }
            return (output, true);
        }
        (output, false)
    }

    fn Leave(&mut self, input: &ast::ExprNode) -> (ast::ExprNode, bool) {
        (input.clone(), true)
    }
}

fn bindExpr(expr: &mut ast::ExprNode, processor: &mut hintProcessor) {
    let (bound, ok) = expr.Accept(&mut bindExprQueries { processor });
    if ok {
        *expr = bound;
    }
}

fn bindTableName(table: &mut ast::TableName, processor: &mut hintProcessor) {
    if processor.blockCounter > 0 {
        table.IndexHints = processor
            .HintsSet
            .indexHints
            .get(processor.indexCounter)
            .cloned()
            .unwrap_or_default();
        processor.indexCounter += 1;
    }
}

fn bindWith(with: &mut ast::WithClause, processor: &mut hintProcessor) {
    for cte in &mut with.CTEs {
        let query = std::mem::replace(&mut cte.Query, Box::new(ast::SelectStmt::default()));
        cte.Query = bindNode(query, processor);
    }
}

fn bindByItems(items: &mut [ast::ByItem], processor: &mut hintProcessor) {
    for item in items {
        bindExpr(&mut item.Expr, processor);
    }
}

fn bindLimit(limit: &mut ast::Limit, processor: &mut hintProcessor) {
    if let Some(offset) = limit.Offset.as_mut() {
        bindExpr(offset, processor);
    }
    if let Some(count) = limit.Count.as_mut() {
        bindExpr(count, processor);
    }
}

fn bindFields(fields: &mut ast::FieldList, processor: &mut hintProcessor) {
    for field in &mut fields.Fields {
        if let Some(expr) = field.Expr.as_mut() {
            bindExpr(expr, processor);
        }
    }
}

fn bindWindowSpec(spec: &mut ast::WindowSpec, processor: &mut hintProcessor) {
    bindByItems(&mut spec.PartitionBy, processor);
    bindByItems(&mut spec.OrderBy, processor);
    if let Some(frame) = spec.Frame.as_mut() {
        if let Some(expr) = frame.Extent.Start.Expr.as_mut() {
            bindExpr(expr, processor);
        }
        if let Some(expr) = frame.Extent.End.Expr.as_mut() {
            bindExpr(expr, processor);
        }
    }
}

/// 将索引 hint 按序写回 TableSource，并绑定子查询。
fn bindTableSource(source: &mut ast::TableSource, processor: &mut hintProcessor) {
    if let Some(query) = source.QuerySource.take() {
        if let Some(node) = query.take() {
            source.QuerySource = Some(ast::NodeRef::new(bindNode(node, processor)));
        }
    } else {
        bindTableName(&mut source.Source, processor);
        if let Some(sample) = source.TableSample.as_mut() {
            if let Some(expr) = sample.Expr.as_mut() {
                bindExpr(expr, processor);
            }
            if let Some(expr) = sample.RepeatableSeed.as_mut() {
                bindExpr(expr, processor);
            }
        }
        if let Some(as_of) = source.AsOf.as_mut() {
            bindExpr(&mut as_of.TsExpr, processor);
        }
    }
}

/// 绑定 ResultSet 变体上的 hint。
fn bindResultSet(result: &mut ast::ResultSetNode, processor: &mut hintProcessor) {
    match result {
        ast::ResultSetNode::TableSource(source) => bindTableSource(source, processor),
        ast::ResultSetNode::Join(join) => bindJoin(join, processor),
    }
}

/// 绑定 Join 左右子树。
fn bindJoin(join: &mut ast::Join, processor: &mut hintProcessor) {
    if let Some(left) = join.Left.as_deref_mut() {
        bindResultSet(left, processor);
    }
    if let Some(right) = join.Right.as_deref_mut() {
        bindResultSet(right, processor);
    }
    if let Some(on) = join.On.as_mut() {
        bindExpr(on, processor);
    }
}

/// 取出下一组表 hint 并推进 `tableCounter`。
fn nextTableHints(processor: &mut hintProcessor) -> Vec<ast::TableOptimizerHint> {
    let hints = processor
        .HintsSet
        .tableHints
        .get(processor.tableCounter)
        .cloned()
        .unwrap_or_default();
    processor.tableCounter += 1;
    hints
}

/// 将 `HintsSet` 中的表/索引 hint 按收集时的顺序写回 AST。
fn bindNode(node: Box<dyn ast::Node>, processor: &mut hintProcessor) -> Box<dyn ast::Node> {
    if node.as_any().is::<ast::SelectStmt>() {
        let mut stmt = node.into_any().downcast::<ast::SelectStmt>().unwrap();
        stmt.TableHints = nextTableHints(processor);
        processor.blockCounter += 1;
        if let Some(with) = stmt.With.as_mut() {
            bindWith(&mut with.borrow_mut(), processor);
        }
        bindFields(&mut stmt.Fields, processor);
        if let Some(from) = stmt.From.as_mut() {
            bindJoin(&mut from.TableRefs, processor);
        }
        if let Some(where_expr) = stmt.Where.as_mut() {
            bindExpr(where_expr, processor);
        }
        bindByItems(&mut stmt.GroupBy, processor);
        if let Some(having) = stmt.Having.as_mut() {
            bindExpr(having, processor);
        }
        for row in &mut stmt.Lists {
            for value in &mut row.Values {
                bindExpr(value, processor);
            }
        }
        for spec in &mut stmt.WindowSpecs {
            bindWindowSpec(spec, processor);
        }
        bindByItems(&mut stmt.OrderBy, processor);
        if let Some(limit) = stmt.Limit.as_mut() {
            bindLimit(limit, processor);
        }
        if let Some(lock_info) = stmt.lock_info.as_mut() {
            for table in &mut lock_info.Tables {
                bindTableName(table, processor);
            }
        }
        stmt.children = stmt
            .children
            .into_iter()
            .map(|child| bindNode(child, processor))
            .collect();
        processor.blockCounter -= 1;
        return stmt;
    }
    if node.as_any().is::<ast::UpdateStmt>() {
        let mut stmt = node.into_any().downcast::<ast::UpdateStmt>().unwrap();
        stmt.TableHints = nextTableHints(processor);
        processor.blockCounter += 1;
        if let Some(from) = stmt.TableRefs.as_mut() {
            bindJoin(&mut from.TableRefs, processor);
        }
        for assignment in &mut stmt.List {
            bindExpr(&mut assignment.Expr, processor);
        }
        if let Some(where_expr) = stmt.Where.as_mut() {
            bindExpr(where_expr, processor);
        }
        bindByItems(&mut stmt.Order, processor);
        if let Some(limit) = stmt.Limit.as_mut() {
            bindLimit(limit, processor);
        }
        for field in &mut stmt.Returning {
            if let Some(expr) = field.Expr.as_mut() {
                bindExpr(expr, processor);
            }
        }
        processor.blockCounter -= 1;
        return stmt;
    }
    if node.as_any().is::<ast::DeleteStmt>() {
        let mut stmt = node.into_any().downcast::<ast::DeleteStmt>().unwrap();
        stmt.TableHints = nextTableHints(processor);
        processor.blockCounter += 1;
        if let Some(from) = stmt.TableRefs.as_mut() {
            bindJoin(&mut from.TableRefs, processor);
        }
        for table in &mut stmt.Tables {
            bindTableName(table, processor);
        }
        if let Some(where_expr) = stmt.Where.as_mut() {
            bindExpr(where_expr, processor);
        }
        bindByItems(&mut stmt.Order, processor);
        if let Some(limit) = stmt.Limit.as_mut() {
            bindLimit(limit, processor);
        }
        for field in &mut stmt.Returning {
            if let Some(expr) = field.Expr.as_mut() {
                bindExpr(expr, processor);
            }
        }
        processor.blockCounter -= 1;
        return stmt;
    }
    if node.as_any().is::<ast::InsertStmt>() {
        let mut stmt = node.into_any().downcast::<ast::InsertStmt>().unwrap();
        stmt.Select = stmt.Select.take().map(|select| bindNode(select, processor));
        if let Some(table) = stmt.Table.as_mut() {
            bindJoin(&mut table.TableRefs, processor);
        }
        for row in &mut stmt.Lists {
            for value in row {
                bindExpr(value, processor);
            }
        }
        for assignment in &mut stmt.OnDuplicate {
            bindExpr(&mut assignment.Expr, processor);
        }
        for field in &mut stmt.Returning {
            if let Some(expr) = field.Expr.as_mut() {
                bindExpr(expr, processor);
            }
        }
        return stmt;
    }
    if node.as_any().is::<ast::SetOprStmt>() {
        let mut stmt = node.into_any().downcast::<ast::SetOprStmt>().unwrap();
        if let Some(with) = stmt.With.as_mut() {
            bindWith(&mut with.borrow_mut(), processor);
        }
        if let Some(with) = stmt.select_list.With.as_mut() {
            bindWith(&mut with.borrow_mut(), processor);
        }
        stmt.select_list.selects = stmt
            .select_list
            .selects
            .into_iter()
            .map(|select| bindNode(select, processor))
            .collect();
        bindByItems(&mut stmt.select_list.OrderBy, processor);
        if let Some(limit) = stmt.select_list.Limit.as_mut() {
            bindLimit(limit, processor);
        }
        bindByItems(&mut stmt.OrderBy, processor);
        if let Some(limit) = stmt.Limit.as_mut() {
            bindLimit(limit, processor);
        }
        return stmt;
    }
    node
}

/// 把给定 hint 集合绑定到语句 AST，返回更新后的根节点。
pub fn BindHint(statement: Box<dyn ast::Node>, hintsSet: HintsSet) -> Box<dyn ast::Node> {
    let mut processor = hintProcessor {
        HintsSet: hintsSet,
        bindHint2Ast: true,
        ..Default::default()
    };
    bindNode(statement, &mut processor)
}

/// 解析 SQL 并收集 hint：规范化 QB 名、补全缺省库名，返回 hint 集、处理后 AST 与警告。
pub fn ParseHintsSet(
    parserState: &mut parser::Parser,
    sql: &str,
    charset: &str,
    collation: &str,
    db: &str,
) -> Result<(HintsSet, Box<dyn ast::Node>, Vec<errors::Error>), errors::Error> {
    let charsetParam = parser::CharsetConnection(charset.to_string());
    let collationParam = parser::CollationConnection(collation.to_string());
    let (mut statements, warns) = parserState.ParseSQL(sql, &[&charsetParam, &collationParam])?;
    if statements.len() != 1 {
        return Err(errors::New(format!(
            "bind_sql must be a single statement: {}",
            sql
        )));
    }
    let statement = statements.pop().unwrap();
    let mut hintsSet = CollectHint(statement.as_ref());
    let topNodeType = nodeType4Stmt(statement.as_ref());
    let mut handler = NewQBHintHandler(None);
    let statement = handler.Process(statement);

    // 按查询块规范化：解析 qb_name、校验 offset、为表补默认数据库名。
    for (index, blockHints) in hintsSet.tableHints.iter_mut().enumerate() {
        let mut currentOffset = index as i32 + 1;
        // DELETE/UPDATE 顶层 offset 从 0 起，相对 SELECT 需减一。
        if matches!(topNodeType, NodeType::TypeDelete | NodeType::TypeUpdate) {
            currentOffset -= 1;
        }
        let mut normalized = Vec::with_capacity(blockHints.len());
        for mut hint in std::mem::take(blockHints) {
            if hint.HintName.L == hintQBName {
                if !hint.Tables.is_empty() {
                    normalized.push(hint);
                }
                continue;
            }
            if handler.isHint4View(&hint) {
                normalized.push(hint);
                continue;
            }
            let offset = handler.GetHintOffset(&hint.QBName, currentOffset);
            if offset < 0 || !handler.checkTableQBName(&hint.Tables) {
                return Err(errors::New(format!(
                    "Unknown query block name in hint {}",
                    RestoreTableOptimizerHint(&hint)
                )));
            }
            hint.QBName = GenerateQBName(topNodeType, offset)?;
            for table in &mut hint.Tables {
                if table.DBName.O.is_empty() {
                    table.DBName = ast::NewCIStr(db);
                }
            }
            normalized.push(hint);
        }
        *blockHints = normalized;
    }
    Ok((hintsSet, statement, extractHintWarns(warns)))
}

/// 从解析警告中筛出与优化器 hint / memory quota 相关的错误。
pub fn extractHintWarns(warns: Vec<parser::errors::Error>) -> Vec<errors::Error> {
    const HINT_WARNING_CODES: [i32; 6] = [
        errno::errcode::ErrWarnOptimizerHintUnsupportedHint as i32,
        errno::errcode::ErrWarnOptimizerHintInvalidToken as i32,
        errno::errcode::ErrWarnMemoryQuotaOverflow as i32,
        errno::errcode::ErrWarnOptimizerHintParseError as i32,
        errno::errcode::ErrWarnOptimizerHintInvalidInteger as i32,
        errno::errcode::ErrWarnOptimizerHintWrongPos as i32,
    ];
    for warning in warns {
        let is_hint_warning = dbterror::errors::Find(Some(&warning), |candidate| {
            candidate
                .downcast_ref::<dbterror::errors::Error>()
                .is_some_and(|error| {
                    error.RFCCode().starts_with("parser:")
                        && HINT_WARNING_CODES.contains(&error.Code())
                })
        })
        .is_some();
        if parser::ErrParse.Equal(Some(&warning)) || is_hint_warning {
            return vec![warning.into()];
        }
    }
    Vec::new()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
/// 语句顶层类型，用于生成默认查询块名（`sel_N`/`upd_1`/`del_1`）。
pub enum NodeType {
    TypeUpdate = 0,
    TypeDelete = 1,
    TypeSelect = 2,
    TypeInvalid = 3,
}

/// 将 AST 节点映射为 `NodeType`；INSERT 按 SELECT 类型处理。
pub fn nodeType4Stmt(node: &dyn ast::Node) -> NodeType {
    if node.as_any().is::<ast::SelectStmt>() || node.as_any().is::<ast::InsertStmt>() {
        NodeType::TypeSelect
    } else if node.as_any().is::<ast::UpdateStmt>() {
        NodeType::TypeUpdate
    } else if node.as_any().is::<ast::DeleteStmt>() {
        NodeType::TypeDelete
    } else {
        NodeType::TypeInvalid
    }
}

/// 检查历史执行计划自动生成的 binding hint 是否“完整”（表数、子查询、TiFlash）。
struct bindableChecker {
    /// 当前是否仍认为 binding 完整。
    complete: bool,
    /// 不完整时的原因说明。
    reason: String,
    /// 已见到的表（原名/小写名）集合，用于限制多表 Join。
    tables: HashSet<(String, String)>,
}

impl bindableChecker {
    /// 记录表；超过 3 张表的 Join 视为 hint 可能不完整。
    fn visitTable(&mut self, table: &ast::TableName) {
        let schema = (table.Schema.O.clone(), table.Schema.L.clone());
        if !self.tables.contains(&schema) {
            self.tables
                .insert((table.Name.O.clone(), table.Name.L.clone()));
        }
        if self.tables.len() >= 3 {
            self.complete = false;
            self.reason = "auto-generated hint for queries with more than 3 table join might not be complete, the plan might change even after creating this binding".to_string();
        }
    }

    /// 遍历 ResultSet 中的表与子查询。
    fn visitResultSet(&mut self, result: &ast::ResultSetNode) {
        if !self.complete {
            return;
        }
        match result {
            ast::ResultSetNode::TableSource(source) => {
                self.visitTable(&source.Source);
                if let Some(query) = &source.QuerySource {
                    query.with_node(|node| self.visitNode(node));
                }
            }
            ast::ResultSetNode::Join(join) => self.visitJoin(join),
        }
    }

    /// 遍历 Join 左右子树。
    fn visitJoin(&mut self, join: &ast::Join) {
        if let Some(left) = join.Left.as_deref() {
            self.visitResultSet(left);
        }
        if let Some(right) = join.Right.as_deref() {
            self.visitResultSet(right);
        }
    }

    /// 检测子查询表达式；超过限制则标记 incomplete。
    fn visitNode(&mut self, node: &dyn ast::Node) {
        if !self.complete {
            return;
        }
        if let Some(table) = node.as_any().downcast_ref::<ast::TableName>() {
            self.visitTable(table);
            return;
        }
        if let Some(expression) = node.as_any().downcast_ref::<ast::ExprNode>() {
            if matches!(
                expression.Kind,
                ast::ExprKind::Subquery { .. }
                    | ast::ExprKind::CompareSubquery { .. }
                    | ast::ExprKind::InSubquery { .. }
                    | ast::ExprKind::ExistsSubquery { .. }
            ) {
                self.complete = false;
                self.reason = "auto-generated hint for queries with sub queries might not be complete, the plan might change even after creating this binding".to_string();
            }
            return;
        }
        if let Some(stmt) = node.as_any().downcast_ref::<ast::SelectStmt>() {
            if let Some(from) = &stmt.From {
                self.visitJoin(&from.TableRefs);
            }
            for child in &stmt.children {
                self.visitNode(child.as_ref());
            }
            return;
        }
        if let Some(stmt) = node.as_any().downcast_ref::<ast::UpdateStmt>() {
            if let Some(from) = &stmt.TableRefs {
                self.visitJoin(&from.TableRefs);
            }
            return;
        }
        if let Some(stmt) = node.as_any().downcast_ref::<ast::DeleteStmt>() {
            if let Some(from) = &stmt.TableRefs {
                self.visitJoin(&from.TableRefs);
            }
            return;
        }
        if let Some(stmt) = node.as_any().downcast_ref::<ast::InsertStmt>() {
            if let Some(select) = stmt.Select.as_deref() {
                self.visitNode(select);
            }
            return;
        }
        if let Some(stmt) = node.as_any().downcast_ref::<ast::SetOprStmt>() {
            for select in &stmt.select_list.selects {
                self.visitNode(select.as_ref());
            }
        }
    }
}

/// 判断历史计划生成的 hint 是否可安全作为 binding；返回 (完整?, 原因)。
pub fn CheckBindingFromHistoryComplete(node: &dyn ast::Node, hintStr: &str) -> (bool, String) {
    // 访问 TiFlash（列存副本引擎）的自动 hint 可能不完整。
    if hintStr.contains("tiflash") {
        return (
            false,
            "auto-generated hint for queries accessing TiFlash might not be complete, the plan might change even after creating this binding".to_string(),
        );
    }
    let mut checker = bindableChecker {
        complete: true,
        reason: String::new(),
        tables: HashSet::with_capacity(2),
    };
    checker.visitNode(node);
    (checker.complete, checker.reason)
}
