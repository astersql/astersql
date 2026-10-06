// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// Statement hint lifecycle shared by text and prepared execution.
//
// 语句级 Hint（优化器提示）生命周期，供文本 SQL 与预处理执行共用。
// 先应用查询 Hint，若匹配到绑定（Binding）则以其 Hint 覆盖并二次应用 SET_VAR；
// `StatementHintGuard` 在语句结束时回滚会话变量变更。

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::sync::{Arc, Once};

use astersql_parser_ast as ast;
use astersql_sessionctx_variable as variable;
use astersql_util_hint as hint;

/// 进程内只注册一次受限 Hint 检查器。
static INITIALIZE_HINT_RUNTIME: Once = Once::new();

/// 将语义版本受限 Hint 检查结果转为 hint 错误类型。
fn restricted_hint_checker(name: String) -> Option<hint::errors::Error> {
    astersql_util_sem_v2::IsRestrictedHint(&name)
        .err()
        .map(hint::errors::NewNoStackError)
}

/// 初始化 Hint 运行时：注册受限 Hint 检查器（幂等）。
pub fn InitializeHintRuntime() {
    INITIALIZE_HINT_RUNTIME.call_once(|| {
        hint::RegisterRestrictedHintChecker(restricted_hint_checker);
    });
}

/// 解析表优化器 Hint 列表为 `StmtHints`，并收集警告字符串。
fn parse_statement_hints(
    hints: Vec<ast::TableOptimizerHint>,
    current_database: &str,
) -> (hint::StmtHints, Vec<astersql_errors::SharedError>) {
    // SET_VAR 检查：系统变量须存在且已验证允许由 Hint 更新。
    let checker = |name: String, hint_name: String| {
        let Some(system_variable) = variable::GetSysVar(&name) else {
            return (
                false,
                Some(hint::errors::NewNoStackError(format!(
                    "unresolved optimizer hint name {hint_name}({name})"
                ))),
            );
        };
        if !system_variable.IsHintUpdatableVerified {
            return (
                true,
                Some(
                    astersql_util_dbterror_plannererrors::ErrNotHintUpdatable
                        .GenWithStackByArgs(&[name.into()])
                        .into(),
                ),
            );
        }
        (true, None)
    };
    // 本切片无目录后端：HYPO_INDEX 一律报错。
    let hypo_checker = |_database: ast::CIStr, _table: ast::CIStr, _column: ast::CIStr| {
        (
            -1,
            Some(hint::errors::NewNoStackError(
                "HYPO_INDEX requires a catalog-backed index checker",
            )),
        )
    };
    let (parsed, _offsets, warnings) =
        hint::ParseStmtHints(hints, checker, hypo_checker, current_database.to_owned(), 1);
    (
        parsed,
        warnings
            .into_iter()
            .map(|warning| match warning {
                hint::errors::Error::Parser(error) | hint::errors::Error::TiDB(error) => error,
            })
            .collect(),
    )
}

/// 将 Hint 中的 SET_VAR 应用到会话变量，并登记恢复点；失败写入警告。
fn apply_set_vars(
    variables: &variable::session::SessionVars,
    hints: &hint::StmtHints,
    warnings: &mut Vec<astersql_errors::SharedError>,
) {
    for (name, value) in &hints.SetVars {
        match variables.SetHintSystemVarWithOldState(name, value) {
            Ok(old_value) => variables.AddHintSystemVarRestore(name, &old_value),
            Err(error) => warnings.push(astersql_errors::SharedError::new(error)),
        }
    }
}

/// RAII statement boundary. Query hints are applied first; selected binding
/// hints replace the effective statement hints and their SET_VAR values are
/// applied second, matching Go's binding precedence.
///
/// RAII 语句边界：先应用查询 Hint；若选中绑定则以其 Hint 覆盖有效 Hint，
/// 并二次应用 SET_VAR，对齐 Go 绑定优先级。
pub struct StatementHintGuard<'a> {
    /// 会话变量句柄。
    variables: &'a variable::session::SessionVars,
    /// 查询文本自身解析出的 Hint。
    query_hints: hint::StmtHints,
    /// 最终生效的 Hint（可能来自绑定覆盖）。
    effective_hints: hint::StmtHints,
    /// 解析/应用过程中积累的警告。
    warnings: Vec<String>,
    binding_sql: Option<String>,
    /// 是否已显式 `Finish`，避免 Drop 重复收尾。
    finished: bool,
}

impl<'a> StatementHintGuard<'a> {
    /// 返回本语句匹配并成功解析的绑定 SQL。
    pub fn BindingSQL(&self) -> Option<&str> {
        self.binding_sql.as_deref()
    }
    /// 返回查询文本解析出的 Hint。
    pub fn QueryHints(&self) -> &hint::StmtHints {
        &self.query_hints
    }

    /// 返回最终生效的 Hint。
    pub fn EffectiveHints(&self) -> &hint::StmtHints {
        &self.effective_hints
    }

    /// 返回本语句边界内积累的警告。
    pub fn Warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Apply the successful-optimization SET_VAR effects for SELECTs that the
    /// runtime evaluates directly, matching planner::Optimize's final phase.
    pub(crate) fn ApplySuccessfulOptimizeEffects(&self) {
        let mut warnings = Vec::new();
        apply_set_vars(self.variables, &self.effective_hints, &mut warnings);
        for warning in warnings {
            self.variables.StmtCtx.SetHintWarningFromError(warning);
        }
    }

    /// 正常结束语句边界并提交 Hint 收尾逻辑。
    pub fn Finish(mut self) -> Result<(), variable::VariableError> {
        self.finished = true;
        self.variables.FinishHintStatement()
    }
}

impl Drop for StatementHintGuard<'_> {
    fn drop(&mut self) {
        // 未 Finish 时仍需回滚 SET_VAR，防止会话变量泄漏到下一条语句。
        if !self.finished {
            let _ = self.variables.FinishHintStatement();
        }
    }
}

/// 开启语句 Hint 生命周期：解析查询 Hint，可选绑定覆盖，写入 StmtCtx。
pub fn StartStatementHints<'a>(
    variables: &'a variable::session::SessionVars,
    statement: &dyn ast::Node,
    selected_binding: Option<&hint::HintsSet>,
) -> StatementHintGuard<'a> {
    InitializeHintRuntime();
    variable::register_builtin_sysvars();
    variables.BeginHintStatement();
    variables.StmtCtx.StmtHints.StoreForceNthPlan(-1);
    variables.StmtCtx.StmtHints.StoreWriteSlowLog(false);

    let query_table_hints = hint::ExtractTableHintsFromStmtNode(statement, None);
    let (query_hints, mut warnings) =
        parse_statement_hints(query_table_hints, &variables.CurrentDB());
    apply_set_vars(variables, &query_hints, &mut warnings);

    // 绑定优先：用绑定 Hint 替换有效 Hint，并再次应用 SET_VAR。
    let effective_hints = if let Some(binding) = selected_binding {
        let (binding_hints, binding_warnings) =
            parse_statement_hints(binding.GetStmtHints(), &variables.CurrentDB());
        warnings.extend(binding_warnings);
        apply_set_vars(variables, &binding_hints, &mut warnings);
        variables.MarkHintStatementFromBinding();
        binding_hints
    } else {
        query_hints.Clone()
    };

    variables
        .StmtCtx
        .StmtHints
        .StoreForceNthPlan(effective_hints.ForceNthPlan);
    variables
        .StmtCtx
        .StmtHints
        .StoreWriteSlowLog(effective_hints.WriteSlowLog);
    for warning in &warnings {
        variables.StmtCtx.SetHintWarningFromError(warning.clone());
    }

    StatementHintGuard {
        variables,
        query_hints,
        effective_hints,
        warnings: warnings
            .into_iter()
            .map(|warning| warning.to_string())
            .collect(),
        binding_sql: None,
        finished: false,
    }
}

/// 从 AST 收集绑定匹配所需的表名（含 schema/别名）。
fn collect_binding_tables(node: &dyn ast::Node, output: &mut Vec<astersql_bindinfo::TableName>) {
    let Some(select) = node.as_any().downcast_ref::<ast::SelectStmt>() else {
        return;
    };
    let Some(from) = &select.From else { return };
    fn collect_result_set(
        result_set: &ast::ResultSetNode,
        output: &mut Vec<astersql_bindinfo::TableName>,
    ) {
        match result_set {
            ast::ResultSetNode::TableSource(source) => {
                if let Some(query) = &source.QuerySource {
                    let _ = query.with_node(|query| collect_binding_tables(query, output));
                } else {
                    output.push(astersql_bindinfo::TableName {
                        Schema: source.Source.Schema.L.clone(),
                        Name: source.Source.Name.L.clone(),
                        Alias: source.AsName.L.clone(),
                    });
                }
            }
            ast::ResultSetNode::Join(join) => {
                if let Some(left) = join.Left.as_deref() {
                    collect_result_set(left, output);
                }
                if let Some(right) = join.Right.as_deref() {
                    collect_result_set(right, output);
                }
            }
        }
    }
    if let Some(left) = from.TableRefs.Left.as_deref() {
        collect_result_set(left, output);
    }
    if let Some(right) = from.TableRefs.Right.as_deref() {
        collect_result_set(right, output);
    }
}

/// 由 SQL 文本与 AST 构造绑定匹配用的 `Statement` 描述。
pub fn BindingStatementFromAST(
    sql: &str,
    statement: &dyn ast::Node,
) -> astersql_bindinfo::Statement {
    let mut tables = Vec::new();
    collect_binding_tables(statement, &mut tables);
    let mut binding_statement = astersql_bindinfo::Statement {
        SQL: sql.to_owned(),
        Tables: tables,
        HasParamMarker: false,
    };
    let parsed_tables = astersql_bindinfo::CollectTableNames(&astersql_bindinfo::Statement {
        SQL: sql.to_owned(),
        ..Default::default()
    });
    if !parsed_tables.is_empty() {
        binding_statement.Tables = parsed_tables;
    }
    binding_statement.HasParamMarker = astersql_bindinfo::hasParam(&binding_statement);
    binding_statement
}

fn binding_sql_for_warning(binding: &astersql_bindinfo::Binding) -> String {
    let mut parser = astersql_parser::Parser::default();
    let Ok(statement) = parser.ParseOneStmt(&binding.BindSQL, &binding.Charset, &binding.Collation)
    else {
        return binding.BindSQL.clone();
    };
    let restored = astersql_util_parser::RestoreWithDefaultDB(
        statement.as_ref(),
        &binding.Db,
        &binding.BindSQL,
    );
    if restored.is_empty() {
        return binding.BindSQL.clone();
    }
    let hints = hint::ExtractTableHintsFromStmtNode(statement.as_ref(), None);
    if hints.is_empty() {
        return restored;
    }
    let hint_text = hint::RestoreOptimizerHints(hints);
    if let Some(rest) = restored.strip_prefix("SELECT ") {
        return format!("SELECT /*+ {hint_text}*/ {rest}");
    }
    // The generic SQL restorer does not write optimizer comments for other
    // statement forms. Keep their original hint text until those forms have a
    // dedicated canonical restoration path.
    binding.BindSQL.clone()
}

/// Session-owned binding matcher used by the production execution entry.
///
/// 会话持有的绑定目录：会话级/全局级绑定列表 + 匹配缓存。
#[derive(Default)]
pub struct SessionBindingCatalog {
    /// 当前数据库名，用于跨库匹配。
    pub CurrentDB: String,
    /// 是否启用计划基线（plan baselines）。
    pub UsePlanBaselines: bool,
    /// 是否启用跨库通配绑定。
    pub EnableFuzzyBinding: bool,
    /// 是否记录全局绑定最近使用时间。
    pub EnableBindingUsage: bool,
    /// 会话级绑定。
    session: Vec<Arc<astersql_bindinfo::Binding>>,
    /// 全局级绑定。
    global: Vec<Arc<astersql_bindinfo::Binding>>,
    /// 语句键 → 匹配结果缓存。
    cache: HashMap<String, astersql_bindinfo::BindingCacheItem>,
}

impl SessionBindingCatalog {
    /// 以当前库名构造空目录，默认启用计划基线。
    pub fn New(current_database: impl Into<String>) -> Self {
        Self {
            CurrentDB: current_database.into(),
            UsePlanBaselines: true,
            EnableBindingUsage: true,
            ..Default::default()
        }
    }

    fn upsert_binding(
        bindings: &mut Vec<Arc<astersql_bindinfo::Binding>>,
        binding: astersql_bindinfo::Binding,
    ) {
        bindings.retain(|existing| existing.SQLDigest != binding.SQLDigest);
        bindings.push(Arc::new(binding));
    }

    /// 创建或替换会话级绑定并清空匹配缓存。
    pub fn AddSessionBinding(&mut self, binding: astersql_bindinfo::Binding) {
        Self::upsert_binding(&mut self.session, binding);
        self.cache.clear();
    }

    /// 创建或替换全局级绑定并清空匹配缓存。
    pub fn AddGlobalBinding(&mut self, binding: astersql_bindinfo::Binding) {
        Self::upsert_binding(&mut self.global, binding);
        self.cache.clear();
    }

    /// 用 Domain 共享缓存的当前快照替换本会话的全局绑定视图。
    pub fn ReplaceGlobalBindings(&mut self, bindings: Vec<Arc<astersql_bindinfo::Binding>>) {
        if self.global.len() == bindings.len()
            && self
                .global
                .iter()
                .zip(&bindings)
                .all(|(current, replacement)| Arc::ptr_eq(current, replacement))
        {
            return;
        }
        self.global = bindings;
        self.cache.clear();
    }

    /// 返回 SHOW BINDINGS 所需的稳定快照。
    pub fn Bindings(&self, global: bool) -> Vec<Arc<astersql_bindinfo::Binding>> {
        let mut bindings = if global {
            self.global.clone()
        } else {
            self.session.clone()
        };
        bindings.retain(|binding| binding.IsBindingEnabled());
        bindings.sort_by(|left, right| {
            left.OriginalSQL
                .cmp(&right.OriginalSQL)
                .then_with(|| left.BindSQL.cmp(&right.BindSQL))
                .then_with(|| left.SQLDigest.cmp(&right.SQLDigest))
        });
        bindings
    }

    /// 按 SQL digest 删除会话级绑定，返回实际删除数。
    pub fn DropSessionBindings(&mut self, digests: &[String]) -> usize {
        let before = self.session.len();
        self.session
            .retain(|binding| !digests.contains(&binding.SQLDigest));
        let removed = before - self.session.len();
        if removed != 0 {
            self.cache.clear();
        }
        removed
    }

    /// 在给定绑定集合中按 digest 与表名做跨库匹配。
    fn match_bindings(
        &self,
        digest: &str,
        tables: &[astersql_bindinfo::TableName],
        bindings: &[Arc<astersql_bindinfo::Binding>],
        enable_fuzzy_binding: bool,
    ) -> Option<Arc<astersql_bindinfo::Binding>> {
        let candidates = bindings
            .iter()
            .filter(|binding| {
                astersql_bindinfo::noDBDigestFromBinding(binding)
                    .is_ok_and(|candidate| candidate == digest)
            })
            .cloned()
            .collect::<Vec<_>>();
        astersql_bindinfo::crossDBMatchBindingsWithFuzzy(
            &self.CurrentDB,
            tables,
            &candidates,
            enable_fuzzy_binding,
        )
        .0
    }
}

impl astersql_bindinfo::BindingMatchContext for SessionBindingCatalog {
    fn use_plan_baselines(&self) -> bool {
        self.UsePlanBaselines
    }

    fn current_db(&self) -> &str {
        &self.CurrentDB
    }

    fn cached_match(&self, statement_key: &str) -> Option<astersql_bindinfo::BindingCacheItem> {
        self.cache.get(statement_key).cloned()
    }

    fn cache_match(&mut self, statement_key: String, item: astersql_bindinfo::BindingCacheItem) {
        self.cache.insert(statement_key, item);
    }

    fn enable_fuzzy_binding(&self) -> bool {
        self.EnableFuzzyBinding
    }

    fn set_enable_fuzzy_binding(&mut self, enabled: bool) {
        if self.EnableFuzzyBinding != enabled {
            self.EnableFuzzyBinding = enabled;
            self.cache.clear();
        }
    }

    fn enable_binding_usage(&self) -> bool {
        self.EnableBindingUsage
    }

    fn set_enable_binding_usage(&mut self, enabled: bool) {
        self.EnableBindingUsage = enabled;
    }

    fn match_session_binding(
        &self,
        digest: &str,
        tables: &[astersql_bindinfo::TableName],
    ) -> Option<Arc<astersql_bindinfo::Binding>> {
        self.match_bindings(digest, tables, &self.session, self.EnableFuzzyBinding)
    }

    fn match_session_binding_with_fuzzy(
        &self,
        digest: &str,
        tables: &[astersql_bindinfo::TableName],
        enable_fuzzy_binding: bool,
    ) -> Option<Arc<astersql_bindinfo::Binding>> {
        self.match_bindings(digest, tables, &self.session, enable_fuzzy_binding)
    }

    fn match_global_binding(
        &self,
        digest: &str,
        tables: &[astersql_bindinfo::TableName],
    ) -> Option<Arc<astersql_bindinfo::Binding>> {
        self.match_bindings(digest, tables, &self.global, self.EnableFuzzyBinding)
    }

    fn match_global_binding_with_fuzzy(
        &self,
        digest: &str,
        tables: &[astersql_bindinfo::TableName],
        enable_fuzzy_binding: bool,
    ) -> Option<Arc<astersql_bindinfo::Binding>> {
        self.match_bindings(digest, tables, &self.global, enable_fuzzy_binding)
    }
}

/// Performs real bindinfo digest/table matching before entering the hint
/// lifecycle. Callers do not choose a binding or pass a prebuilt HintsSet.
///
/// 进入 Hint 生命周期前先做真实 bindinfo digest/表匹配；
/// 调用方不自行挑选绑定或传入预构建的 `HintsSet`。
pub fn StartStatementHintsWithBindings<'a>(
    variables: &'a variable::session::SessionVars,
    statement: &dyn ast::Node,
    sql: &str,
    bindings: &mut dyn astersql_bindinfo::BindingMatchContext,
) -> StatementHintGuard<'a> {
    let session_enable_fuzzy_binding = variables
        .GetSystemVar(astersql_sessionctx_vardef::TiDBOptEnableFuzzyBinding)
        .is_some_and(|value| {
            value == "1" || value.eq_ignore_ascii_case(astersql_sessionctx_vardef::On)
        });
    // Go applies a statement's SET_VAR override while deciding whether a
    // universal binding is eligible. Parse only enough of the query hints to
    // derive that effective switch; StartStatementHints below still owns the
    // actual apply/restore lifecycle.
    let query_table_hints = hint::ExtractTableHintsFromStmtNode(statement, None);
    let (query_hints, _) = parse_statement_hints(query_table_hints, &variables.CurrentDB());
    let enable_fuzzy_binding = query_hints
        .SetVars
        .get(astersql_sessionctx_vardef::TiDBOptEnableFuzzyBinding)
        .map(|value| value == "1" || value.eq_ignore_ascii_case(astersql_sessionctx_vardef::On))
        .unwrap_or(session_enable_fuzzy_binding);
    bindings.set_enable_fuzzy_binding(enable_fuzzy_binding);
    bindings.set_enable_binding_usage(astersql_sessionctx_vardef::EnableBindingUsage.Load());
    let binding_statement = BindingStatementFromAST(sql, statement);
    let (binding, matched, _scope) =
        astersql_bindinfo::MatchSQLBinding(bindings, &binding_statement);
    let mut binding_parse_warnings = Vec::new();
    // 匹配成功则解析绑定 SQL 中的 Hint 集合；解析失败记入警告并视为无绑定。
    let binding_sql = binding
        .as_ref()
        .map(|binding| binding_sql_for_warning(binding));
    let parsed_binding = binding.and_then(|binding| {
        let mut parser = astersql_parser::Parser::default();
        let binding_db = if binding.TableNames.iter().any(|table| table.Schema == "*") {
            "*"
        } else {
            &binding.Db
        };
        match hint::ParseHintsSet(
            &mut parser,
            &binding.BindSQL,
            &binding.Charset,
            &binding.Collation,
            binding_db,
        ) {
            Ok((hints, _statement, warnings)) => {
                binding_parse_warnings
                    .extend(warnings.into_iter().map(|warning| warning.to_string()));
                Some(hints)
            }
            Err(error) => {
                binding_parse_warnings.push(error.to_string());
                None
            }
        }
    });
    let mut guard = StartStatementHints(
        variables,
        statement,
        if matched {
            parsed_binding.as_ref()
        } else {
            None
        },
    );
    if matched && parsed_binding.is_some() {
        guard.binding_sql = binding_sql;
    }
    for warning in binding_parse_warnings {
        variables.StmtCtx.SetHintWarning(warning.clone());
        guard.warnings.push(warning);
    }
    guard
}
