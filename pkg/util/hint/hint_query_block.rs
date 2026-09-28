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

// 查询块（Query Block）Hint 处理。
//
// 查询块是 SQL 中可独立优化的子查询/语句单元（如 `sel_1`、`upd_1`）。
// 本模块维护 `QB_NAME` 到 SELECT offset 的映射、视图相关 hint 拆分，
// 并在计划构建时按 offset 分发当前语句可用的优化器 hint。

use std::collections::{HashMap, HashSet};

use crate::hint_processor::{NodeType, RestoreTableOptimizerHint};
use crate::{ast, errors};

/// Receives optimizer-hint warnings while query-block metadata is processed.
/// 接收查询块元数据处理过程中产生的优化器 hint 警告。
pub trait hintWarnHandler {
    /// 记录一条文本警告。
    fn SetHintWarning(&mut self, warn: String);
    /// 从错误对象提取并记录警告。
    fn SetHintWarningFromError(&mut self, err: &dyn std::error::Error);
}

/// Handles hints that name a query block explicitly or through `sel_N` aliases.
/// 处理显式命名或通过 `sel_N` 别名引用的查询块 hint。
#[derive(Default)]
pub struct QBHintHandler {
    /// 查询块名 → SELECT 语句 offset（从 1 起；Update/Delete 顶层为 0）。
    pub QBNameToSelOffset: HashMap<String, i32>,
    /// 视图 QB 名 → 视图 hint 中声明的表列表。
    pub ViewQBNameToTable: HashMap<String, Vec<ast::HintTable>>,
    /// 视图 QB 名 → 归属该视图的 hint 列表。
    pub ViewQBNameToHints: HashMap<String, Vec<ast::TableOptimizerHint>>,
    /// 可选警告回调。
    pub warnHandler: Option<Box<dyn hintWarnHandler>>,
    /// 已遍历到的 SELECT 语句计数（即最大 offset）。
    pub selectStmtOffset: i32,
}

struct processExprQueries<'a> {
    handler: &'a mut QBHintHandler,
}

impl ast::ExprNodeVisitor for processExprQueries<'_> {
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
                    Query: ast::NodeRef::new(self.handler.Process(node)),
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

/// Runtime state belongs to one plan build rather than to the reusable handler.
/// 单次计划构建的运行时状态（与可复用的 Handler 分离）。
#[derive(Default)]
pub struct QBHintBuildState {
    /// 各查询块 offset → 分配给该块的 hint。
    pub QBOffsetToHints: HashMap<i32, Vec<ast::TableOptimizerHint>>,
    /// 本次构建中实际用到的视图 QB 名集合（用于检测未使用的 qb_name）。
    pub ViewQBNameUsed: Option<HashSet<String>>,
}

/// 构造带可选警告处理器的 `QBHintHandler`。
pub fn NewQBHintHandler(warnHandler: Option<Box<dyn hintWarnHandler>>) -> QBHintHandler {
    QBHintHandler {
        warnHandler,
        ..Default::default()
    }
}

/// `qb_name` hint 的小写名称常量。
pub const hintQBName: &str = "qb_name";
/// Update 语句默认查询块名（offset 0）。
pub const defaultUpdateBlockName: &str = "upd_1";
/// Delete 语句默认查询块名（offset 0）。
pub const defaultDeleteBlockName: &str = "del_1";
/// SELECT 查询块名前缀，后缀为 offset 数字。
pub const defaultSelectBlockPrefix: &str = "sel_";

impl QBHintHandler {
    /// 为一次计划构建初始化状态；若存在视图 QB 则预分配 used 集合。
    pub fn NewBuildState(&self) -> QBHintBuildState {
        let mut state = QBHintBuildState::default();
        if !self.ViewQBNameToTable.is_empty() {
            state.ViewQBNameUsed = Some(HashSet::with_capacity(self.ViewQBNameToTable.len()));
        }
        state
    }

    /// 返回已处理的最大 SELECT offset。
    pub fn MaxSelectStmtOffset(&self) -> i32 {
        self.selectStmtOffset
    }

    /// Processes an owned AST and returns it after applying the same mutations as
    /// the Go visitor. The parser integration exposes immutable visitor nodes, so
    /// ownership is used for safe in-place replacement instead of interior casts.
    /// 遍历 AST：为 SELECT 编号、拆分视图 hint、校验 QB_NAME，并递归子节点。
    pub fn Process(&mut self, node: Box<dyn ast::Node>) -> Box<dyn ast::Node> {
        // Explain / CreateBinding 不做查询块处理，原样返回。
        if node.as_any().is::<ast::ExplainStmt>() || node.as_any().is::<ast::CreateBindingStmt>() {
            return node;
        }
        if node.as_any().is::<ast::UpdateStmt>() {
            let mut stmt = node.into_any().downcast::<ast::UpdateStmt>().unwrap();
            // Update 顶层 offset 为 0。
            self.checkQueryBlockHints(&stmt.TableHints, 0);
            if let Some(with) = stmt.With.as_mut() {
                self.processWith(&mut with.borrow_mut());
            }
            if let Some(tables) = stmt.TableRefs.as_mut() {
                self.processJoin(&mut tables.TableRefs);
            }
            for assignment in &mut stmt.List {
                self.processExpr(&mut assignment.Expr);
            }
            if let Some(where_expr) = stmt.Where.as_mut() {
                self.processExpr(where_expr);
            }
            self.processByItems(&mut stmt.Order);
            if let Some(limit) = stmt.Limit.as_mut() {
                self.processLimit(limit);
            }
            self.processFields(&mut stmt.Returning);
            return stmt;
        }
        if node.as_any().is::<ast::DeleteStmt>() {
            let mut stmt = node.into_any().downcast::<ast::DeleteStmt>().unwrap();
            self.checkQueryBlockHints(&stmt.TableHints, 0);
            if let Some(with) = stmt.With.as_mut() {
                self.processWith(&mut with.borrow_mut());
            }
            if let Some(tables) = stmt.TableRefs.as_mut() {
                self.processJoin(&mut tables.TableRefs);
            }
            if let Some(where_expr) = stmt.Where.as_mut() {
                self.processExpr(where_expr);
            }
            self.processByItems(&mut stmt.Order);
            if let Some(limit) = stmt.Limit.as_mut() {
                self.processLimit(limit);
            }
            self.processFields(&mut stmt.Returning);
            return stmt;
        }
        if node.as_any().is::<ast::SelectStmt>() {
            let mut stmt = node.into_any().downcast::<ast::SelectStmt>().unwrap();
            // 每个 SELECT 递增 offset，写入 QueryBlockOffset 供后续计划使用。
            self.selectStmtOffset += 1;
            stmt.QueryBlockOffset = self.selectStmtOffset as isize;
            stmt.TableHints =
                self.handleViewHints(std::mem::take(&mut stmt.TableHints), self.selectStmtOffset);
            self.checkQueryBlockHints(&stmt.TableHints, self.selectStmtOffset);
            if let Some(with) = stmt.With.as_mut() {
                self.processWith(&mut with.borrow_mut());
            }
            self.processFieldList(&mut stmt.Fields);
            if let Some(tables) = stmt.From.as_mut() {
                self.processJoin(&mut tables.TableRefs);
            }
            if let Some(where_expr) = stmt.Where.as_mut() {
                self.processExpr(where_expr);
            }
            self.processByItems(&mut stmt.GroupBy);
            if let Some(having) = stmt.Having.as_mut() {
                self.processExpr(having);
            }
            for row in &mut stmt.Lists {
                for value in &mut row.Values {
                    self.processExpr(value);
                }
            }
            for spec in &mut stmt.WindowSpecs {
                self.processWindowSpec(spec);
            }
            self.processByItems(&mut stmt.OrderBy);
            if let Some(limit) = stmt.Limit.as_mut() {
                self.processLimit(limit);
            }
            stmt.children = stmt
                .children
                .into_iter()
                .map(|child| self.Process(child))
                .collect();
            return stmt;
        }
        if node.as_any().is::<ast::InsertStmt>() {
            let mut stmt = node.into_any().downcast::<ast::InsertStmt>().unwrap();
            // INSERT ... SELECT 先处理 SELECT，随后按 Go Accept 顺序处理其余表达式。
            stmt.Select = stmt.Select.take().map(|select| self.Process(select));
            if let Some(table) = stmt.Table.as_mut() {
                self.processJoin(&mut table.TableRefs);
            }
            for row in &mut stmt.Lists {
                for value in row {
                    self.processExpr(value);
                }
            }
            for assignment in &mut stmt.OnDuplicate {
                self.processExpr(&mut assignment.Expr);
            }
            self.processFields(&mut stmt.Returning);
            return stmt;
        }
        if node.as_any().is::<ast::SetOprStmt>() {
            let mut stmt = node.into_any().downcast::<ast::SetOprStmt>().unwrap();
            // UNION 等集合运算：分别处理各分支 SELECT。
            if let Some(with) = stmt.With.as_mut() {
                self.processWith(&mut with.borrow_mut());
            }
            if let Some(with) = stmt.select_list.With.as_mut() {
                self.processWith(&mut with.borrow_mut());
            }
            stmt.select_list.selects = stmt
                .select_list
                .selects
                .into_iter()
                .map(|select| self.Process(select))
                .collect();
            self.processByItems(&mut stmt.select_list.OrderBy);
            if let Some(limit) = stmt.select_list.Limit.as_mut() {
                self.processLimit(limit);
            }
            self.processByItems(&mut stmt.OrderBy);
            if let Some(limit) = stmt.Limit.as_mut() {
                self.processLimit(limit);
            }
            return stmt;
        }
        node
    }

    fn processExpr(&mut self, expr: &mut ast::ExprNode) {
        let (processed, ok) = expr.Accept(&mut processExprQueries { handler: self });
        if ok {
            *expr = processed;
        }
    }

    fn processWith(&mut self, with: &mut ast::WithClause) {
        for cte in &mut with.CTEs {
            let query = std::mem::replace(&mut cte.Query, Box::new(ast::SelectStmt::default()));
            cte.Query = self.Process(query);
        }
    }

    fn processByItems(&mut self, items: &mut [ast::ByItem]) {
        for item in items {
            self.processExpr(&mut item.Expr);
        }
    }

    fn processLimit(&mut self, limit: &mut ast::Limit) {
        if let Some(offset) = limit.Offset.as_mut() {
            self.processExpr(offset);
        }
        if let Some(count) = limit.Count.as_mut() {
            self.processExpr(count);
        }
    }

    fn processFields(&mut self, fields: &mut [ast::SelectField]) {
        for field in fields {
            if let Some(expr) = field.Expr.as_mut() {
                self.processExpr(expr);
            }
        }
    }

    fn processFieldList(&mut self, fields: &mut ast::FieldList) {
        self.processFields(&mut fields.Fields);
    }

    fn processWindowSpec(&mut self, spec: &mut ast::WindowSpec) {
        self.processByItems(&mut spec.PartitionBy);
        self.processByItems(&mut spec.OrderBy);
        if let Some(frame) = spec.Frame.as_mut() {
            if let Some(expr) = frame.Extent.Start.Expr.as_mut() {
                self.processExpr(expr);
            }
            if let Some(expr) = frame.Extent.End.Expr.as_mut() {
                self.processExpr(expr);
            }
        }
    }

    /// 处理派生表/子查询源：递归 Process 内层查询。
    fn processTableSource(&mut self, source: &mut ast::TableSource) {
        let Some(query) = source.QuerySource.take() else {
            if let Some(sample) = source.TableSample.as_mut() {
                if let Some(expr) = sample.Expr.as_mut() {
                    self.processExpr(expr);
                }
                if let Some(expr) = sample.RepeatableSeed.as_mut() {
                    self.processExpr(expr);
                }
            }
            if let Some(as_of) = source.AsOf.as_mut() {
                self.processExpr(&mut as_of.TsExpr);
            }
            return;
        };
        if let Some(node) = query.take() {
            source.QuerySource = Some(ast::NodeRef::new(self.Process(node)));
        }
    }

    /// 按 ResultSet 节点类型分派到表源或 Join。
    fn processResultSet(&mut self, result: &mut ast::ResultSetNode) {
        match result {
            ast::ResultSetNode::TableSource(source) => self.processTableSource(source),
            ast::ResultSetNode::Join(join) => self.processJoin(join),
        }
    }

    /// 递归处理 Join 左右子树。
    fn processJoin(&mut self, join: &mut ast::Join) {
        if let Some(left) = join.Left.as_deref_mut() {
            self.processResultSet(left);
        }
        if let Some(right) = join.Right.as_deref_mut() {
            self.processResultSet(right);
        }
        if let Some(on) = join.On.as_mut() {
            self.processExpr(on);
        }
    }

    /// 登记本查询块内的 `qb_name`；重复名只保留首次并告警。
    pub fn checkQueryBlockHints(&mut self, hints: &[ast::TableOptimizerHint], offset: i32) {
        let mut qbName = String::new();
        for hint in hints {
            if hint.HintName.L != hintQBName {
                continue;
            }
            // 同一块内多个 qb_name：保留第一个，其余告警。
            if !qbName.is_empty() {
                if let Some(handler) = self.warnHandler.as_mut() {
                    handler.SetHintWarning(format!(
                        "There are more than two query names in same query block, using the first one {}",
                        qbName
                    ));
                }
            } else {
                qbName = hint.QBName.L.clone();
            }
        }
        if qbName.is_empty() {
            return;
        }
        if self.QBNameToSelOffset.contains_key(&qbName) {
            if let Some(handler) = self.warnHandler.as_mut() {
                handler.SetHintWarning(format!(
                    "Duplicate query block name {}, only the first one is effective",
                    qbName
                ));
            }
        } else {
            self.QBNameToSelOffset.insert(qbName, offset);
        }
    }

    /// 从当前 SELECT 的 hint 列表中拆出视图相关 hint，返回剩余普通 hint。
    pub fn handleViewHints(
        &mut self,
        mut hints: Vec<ast::TableOptimizerHint>,
        offset: i32,
    ) -> Vec<ast::TableOptimizerHint> {
        if hints.is_empty() {
            return Vec::new();
        }

        let mut usedHints = vec![false; hints.len()];
        // 第一遍：登记带表列表的 qb_name（视图查询块定义）。
        for (index, hint) in hints.iter_mut().enumerate() {
            if hint.HintName.L != hintQBName || hint.Tables.is_empty() {
                continue;
            }
            usedHints[index] = true;
            let qbName = hint.QBName.L.clone();
            if qbName.is_empty() {
                continue;
            }
            if self.ViewQBNameToTable.contains_key(&qbName) {
                if let Some(handler) = self.warnHandler.as_mut() {
                    handler.SetHintWarning(format!(
                        "Duplicate query block name {} for view's query block hint, only the first one is effective",
                        qbName
                    ));
                }
            } else {
                // 非顶层 SELECT：若表未写 QBName，默认填 sel_<offset>。
                if offset != 1 && hint.Tables[0].QBName.L.is_empty() {
                    hint.Tables[0].QBName =
                        ast::NewCIStr(&format!("{}{}", defaultSelectBlockPrefix, offset));
                }
                self.ViewQBNameToTable.insert(qbName, hint.Tables.clone());
            }
        }

        // 第二遍：归属视图的其它 hint 挂到 ViewQBNameToHints。
        for (index, hint) in hints.iter().enumerate() {
            if usedHints[index] || hint.HintName.L == hintQBName {
                continue;
            }

            let mut valid = false;
            let mut qbName = hint.QBName.L.clone();
            if !qbName.is_empty() {
                valid = self.ViewQBNameToTable.contains_key(&qbName);
            } else if let Some(first) = hint.Tables.first() {
                qbName = first.QBName.L.clone();
                valid = self.ViewQBNameToTable.contains_key(&qbName);
                // 视图 hint 的多表必须共用同一 QB 名。
                if valid && hint.Tables.iter().any(|table| table.QBName.L != qbName) {
                    valid = false;
                    if let Some(handler) = self.warnHandler.as_mut() {
                        handler.SetHintWarning(
                            "Only one query block name is allowed in a view hint, otherwise the hint will be invalid".to_string(),
                        );
                    }
                    usedHints[index] = true;
                }
            }

            if valid {
                usedHints[index] = true;
                self.ViewQBNameToHints
                    .entry(qbName)
                    .or_default()
                    .push(hint.clone());
            }
        }

        // 过滤已消费的视图 hint，剩余返回给当前 SELECT。
        hints
            .into_iter()
            .enumerate()
            .filter_map(|(index, hint)| (!usedHints[index]).then_some(hint))
            .collect()
    }

    /// 收集未在本次构建中使用的视图 qb_name 警告。
    pub fn HandleUnusedViewHints(
        &mut self,
        state: Option<&QBHintBuildState>,
        mut warns: Vec<String>,
    ) -> Vec<String> {
        let Some(state) = state else {
            return warns;
        };
        warns.clear();
        for qbName in self.ViewQBNameToTable.keys() {
            let used = state
                .ViewQBNameUsed
                .as_ref()
                .is_some_and(|used| used.contains(qbName));
            if !used && self.warnHandler.is_some() {
                warns.push(format!(
                    "The qb_name hint {} is unused, please check whether the table list in the qb_name hint {} is correct",
                    qbName, qbName
                ));
            }
        }
        warns
    }

    /// 将块名解析为 offset：查表、默认 upd/del，或解析 `sel_N` 后缀。
    fn getBlockOffset(&self, blockName: &ast::CIStr) -> i32 {
        if let Some(offset) = self.QBNameToSelOffset.get(&blockName.L) {
            return *offset;
        }
        if blockName.L == defaultUpdateBlockName || blockName.L == defaultDeleteBlockName {
            return 0;
        }
        let Some(suffix) = blockName.L.strip_prefix(defaultSelectBlockPrefix) else {
            return -1;
        };
        let Ok(level) = suffix.parse::<i64>() else {
            return -1;
        };
        // 超出已见 SELECT 范围或下溢则视为非法。
        if level > self.selectStmtOffset as i64 || level < i32::MIN as i64 {
            return -1;
        }
        level as i32
    }

    /// 将一组警告文本写入 warnHandler。
    pub fn SetWarns(&mut self, warns: &[String]) {
        let Some(handler) = self.warnHandler.as_mut() else {
            return;
        };
        for warning in warns {
            handler.SetHintWarning(warning.clone());
        }
    }

    /// 解析 hint 上的 QBName；空则使用当前 offset。
    pub fn GetHintOffset(&self, qbName: &ast::CIStr, currentOffset: i32) -> i32 {
        if qbName.L.is_empty() {
            currentOffset
        } else {
            self.getBlockOffset(qbName)
        }
    }

    /// 检查 hint 中各表的 QBName 均可解析为合法 offset。
    pub fn checkTableQBName(&self, tables: &[ast::HintTable]) -> bool {
        tables
            .iter()
            .all(|table| table.QBName.L.is_empty() || self.getBlockOffset(&table.QBName) >= 0)
    }

    /// 判断 hint 是否归属视图查询块（通过 QBName 或表上的 QBName）。
    pub fn isHint4View(&self, hint: &ast::TableOptimizerHint) -> bool {
        if !hint.QBName.L.is_empty() {
            return self.ViewQBNameToTable.contains_key(&hint.QBName.L);
        }
        hint.Tables
            .iter()
            .all(|table| self.ViewQBNameToTable.contains_key(&table.QBName.L))
    }

    /// 按 QB offset 分发 hint，返回当前 offset 应生效的 hint 列表。
    pub fn GetCurrentStmtHints(
        &mut self,
        hints: &[ast::TableOptimizerHint],
        currentOffset: i32,
        state: Option<&mut QBHintBuildState>,
    ) -> Vec<ast::TableOptimizerHint> {
        let mut localState = QBHintBuildState::default();
        let state = state.unwrap_or(&mut localState);
        for hint in hints {
            if hint.HintName.L == hintQBName {
                continue;
            }
            let offset = self.GetHintOffset(&hint.QBName, currentOffset);
            // 未知查询块名：告警并忽略。
            if offset < 0 || !self.checkTableQBName(&hint.Tables) {
                let hintString = RestoreTableOptimizerHint(hint);
                if let Some(handler) = self.warnHandler.as_mut() {
                    handler.SetHintWarning(format!(
                        "Hint {} is ignored due to unknown query block name",
                        hintString
                    ));
                }
                continue;
            }
            let entry = state.QBOffsetToHints.entry(offset).or_default();
            // 去重：同一 hint 只保留一份。
            if !entry.contains(hint) {
                entry.push(hint.clone());
            }
        }
        state
            .QBOffsetToHints
            .get(&currentOffset)
            .cloned()
            .unwrap_or_default()
    }

    /// 标记某个视图 QB 名在本次构建中已使用。
    pub fn MarkViewQBNameUsed(qbName: &str, state: Option<&mut QBHintBuildState>) {
        if let Some(used) = state.and_then(|state| state.ViewQBNameUsed.as_mut()) {
            used.insert(qbName.to_string());
        }
    }
}

/// 按语句类型与 offset 生成规范查询块名（`sel_N` / `upd_1` / `del_1`）。
pub fn GenerateQBName(nodeType: NodeType, qbOffset: i32) -> Result<ast::CIStr, errors::Error> {
    if qbOffset == 0 {
        return match nodeType {
            NodeType::TypeDelete => Ok(ast::NewCIStr(defaultDeleteBlockName)),
            NodeType::TypeUpdate => Ok(ast::NewCIStr(defaultUpdateBlockName)),
            _ => Err(errors::New(format!(
                "Unexpected NodeType {} when block offset is 0",
                nodeType as i32
            ))),
        };
    }
    Ok(ast::NewCIStr(&format!(
        "{}{}",
        defaultSelectBlockPrefix, qbOffset
    )))
}
