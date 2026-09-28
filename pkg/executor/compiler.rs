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

// SQL 编译器：将 AST（抽象语法树）语句编译为可执行的物理计划 `ExecStmt`。
//
// 流水线：预处理（preprocess）→ 解析/计划缓存 → 优化（optimize）→ 指标计数 →
// 优先级下调判定 → 计划追踪 → PointGet 复用 → 事务预热 → 构建执行语句。
// 依赖通过 `CompilerDependencies` 注入，便于测试与适配真实会话/优化器。
use std::any::Any;
use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::Arc;

use crate::adapter::ExecStmt;
use astersql_errors as errors;
use astersql_parser_ast as ast;
use astersql_planner_core as plannercore;
use astersql_planner_core_base as base;
use astersql_planner_core_resolve as resolve;
use astersql_types::metadata as types;

/// 编译期上下文句柄（追踪、区域等），不透明包装。
#[derive(Clone)]
pub struct CompilerContext(pub Arc<dyn Any + Send + Sync>);

impl Default for CompilerContext {
    fn default() -> Self {
        Self(Arc::new(()))
    }
}

/// 编译所用会话句柄。
#[derive(Clone)]
pub struct CompilerSession(pub Arc<dyn Any + Send + Sync>);

impl Default for CompilerSession {
    fn default() -> Self {
        Self(Arc::new(()))
    }
}

/// 编译区域：结束时释放区域相关资源（对齐 Go CompileRegion.End）。
pub trait CompileRegion {
    fn End(&mut self);
}

/// RAII 守卫：Drop 时调用 CompileRegion::End。
struct CompileRegionGuard(Option<Box<dyn CompileRegion>>);

impl Drop for CompileRegionGuard {
    fn drop(&mut self) {
        if let Some(region) = self.0.as_mut() {
            region.End();
        }
    }
}

/// 编译期可见的信息模式（Information Schema）抽象。
pub trait CompilerInfoSchema: Send {}

/// 预处理结果：解析上下文与快照时间戳。
pub struct CompilerPreprocessResult {
    /// 名称解析等 resolve 上下文。
    pub resolveContext: resolve::Context,
    /// 最近一次快照 TS（Timestamp，MVCC 读版本）。
    pub lastSnapshotTS: u64,
}

/// 已准备语句（prepared statement）的缓存与视图。
pub struct CompilerPreparedStatement {
    /// 计划缓存条目。
    pub cache: Box<plannercore::PlanCacheStmt>,
    /// 语句类型与涉及库的视图。
    pub statementView: CompilerStatementView,
}

/// 优化产物：物理/逻辑计划与输出列名。
pub struct CompilerOptimizeResult {
    /// 优化后的计划树。
    pub plan: Box<dyn base::Plan>,
    /// 结果集字段名。
    pub outputNames: Vec<types::FieldName>,
}

/// 编译时需要的会话状态快照。
pub struct CompilerSessionState {
    /// 是否为内部受限 SQL（不计用户指标）。
    pub inRestrictedSQL: bool,
    /// 资源组名。
    pub resourceGroup: String,
    /// 语句优先级。
    pub priority: i32,
    /// 连接 ID。
    pub connectionID: u64,
    /// 是否对日志中的计划做脱敏。
    pub redactLog: bool,
}

/// 计划追踪信息（归一化计划与 digest）。
pub struct CompilerPlanTrace {
    /// 归一化后的计划文本。
    pub normalizedPlan: String,
    /// 计划 digest 二进制。
    pub digest: Option<Vec<u8>>,
    /// 语句类型标签。
    pub statementType: String,
}

/// 语句指标采集模式。
pub struct CompilerMetricMode {
    /// 是否按库记录 QPS。
    pub recordQPSByDB: bool,
    /// 是否在指标标签中带库名。
    pub recordDBLabel: bool,
}

/// 构建 `ExecStmt` 的输入打包。
pub struct ExecStmtBuildInput {
    pub context: CompilerContext,
    pub infoSchema: Box<dyn CompilerInfoSchema>,
    pub plan: Box<dyn base::Plan>,
    pub lowerPriority: bool,
    pub statement: ast::NodeRef,
    pub session: CompilerSession,
    pub outputNames: Vec<types::FieldName>,
    pub prepared: Option<Box<plannercore::PlanCacheStmt>>,
}

/// 物理计划子树，用于估算行数判定是否降低优先级。
#[derive(Clone)]
pub struct PhysicalPriorityPlan {
    /// 估算行数。
    pub estimatedRows: f64,
    /// 子计划。
    pub children: Vec<PhysicalPriorityPlan>,
}

/// 优先级判定用的计划视图（物理/EXECUTE/DML 等）。
#[derive(Clone)]
pub enum PriorityPlan {
    Physical(PhysicalPriorityPlan),
    Execute(Box<PriorityPlan>),
    Insert(Option<PhysicalPriorityPlan>),
    Delete(Option<PhysicalPriorityPlan>),
    Update(Option<PhysicalPriorityPlan>),
    Other,
}

/// 从 AST/结果节点抽取库名时的树形视图。
#[derive(Clone)]
pub enum ResultNodeView {
    TableSource(Box<ResultNodeView>),
    Select(Option<Box<ResultNodeView>>),
    TableName(Option<String>),
    Join {
        left: Option<Box<ResultNodeView>>,
        right: Option<Box<ResultNodeView>>,
    },
    Other,
}

/// 语句涉及数据库的分类视图。
#[derive(Clone)]
pub enum StatementDatabaseView {
    DirectTable(Option<String>),
    Insert {
        table: Option<ResultNodeView>,
        select: Option<ResultNodeView>,
    },
    Rename(Vec<Option<String>>),
    Tables(Vec<String>),
    Result(Option<ResultNodeView>),
    UpdateOrDelete(Option<ResultNodeView>),
    Call(Option<String>),
    Show {
        database: String,
        table: Option<String>,
    },
    NonTransactional(Option<String>),
    Use(String),
    Binding {
        origin: Option<ResultNodeView>,
        hinted: Option<ResultNodeView>,
    },
    Other,
}

/// 语句类型与数据库归属视图。
#[derive(Clone)]
pub struct CompilerStatementView {
    pub statementType: String,
    pub databases: StatementDatabaseView,
}

/// 编译器全部外部依赖：规划器、会话、事务、指标、追踪与 AST 下沉。
/// 任一阶段无静默成功回退，生产适配器不得省略编译步骤。
/// All operations that touch planner, session, transaction, metrics, tracing,
/// or concrete AST downcasts are required here. No operation has a successful
/// fallback, so a production adapter cannot silently omit a compiler stage.
pub trait CompilerDependencies {
    fn StartCompileRegion(
        &mut self,
        ctx: CompilerContext,
    ) -> (Box<dyn CompileRegion>, CompilerContext);
    fn StatementText(&self, statement: &ast::NodeRef) -> String;
    fn SetStatementReadOnly(&mut self, session: &CompilerSession, statement: &ast::NodeRef);
    fn Preprocess(
        &mut self,
        ctx: CompilerContext,
        session: &CompilerSession,
        statement: &ast::NodeRef,
    ) -> Result<CompilerPreprocessResult, errors::Error>;
    fn AssertTransactionState(
        &mut self,
        session: &CompilerSession,
        result: &CompilerPreprocessResult,
    );
    fn TransactionInfoSchema(&mut self, session: &CompilerSession) -> Box<dyn CompilerInfoSchema>;
    fn SessionState(&self, session: &CompilerSession) -> CompilerSessionState;
    fn PreparedStatement(
        &mut self,
        statement: &ast::NodeRef,
        session: &CompilerSession,
        resolve_context: &resolve::Context,
    ) -> Result<Option<CompilerPreparedStatement>, errors::Error>;
    fn Optimize(
        &mut self,
        ctx: CompilerContext,
        session: &CompilerSession,
        statement: &ast::NodeRef,
        resolve_context: &resolve::Context,
        info_schema: &dyn CompilerInfoSchema,
    ) -> Result<CompilerOptimizeResult, errors::Error>;
    fn AssertStatementStaleness(&mut self, session: &CompilerSession);
    fn StatementView(
        &self,
        statement: &ast::NodeRef,
        resolve_context: &resolve::Context,
    ) -> CompilerStatementView;
    fn MetricMode(&self) -> CompilerMetricMode;
    fn IncrementDBStatement(&mut self, database: &str, statement_type: &str);
    fn IncrementStatement(&mut self, statement_type: &str, database: &str, resource_group: &str);
    fn PriorityPlan(&self, plan: &dyn base::Plan) -> PriorityPlan;
    fn ExpensiveThreshold(&self) -> i64;
    fn SetPlan(&mut self, session: &CompilerSession, plan: &dyn base::Plan);
    fn PlanTrace(
        &self,
        ctx: &CompilerContext,
        session: &CompilerSession,
        statement: &ast::NodeRef,
    ) -> Option<CompilerPlanTrace>;
    fn RedactNormalizedPlan(&self, redact: bool, normalized: &str) -> String;
    fn EmitPlanTrace(
        &mut self,
        ctx: &CompilerContext,
        digest_hex: &str,
        statement_type: &str,
        connection_id: u64,
        normalized_plan: Option<&str>,
    );
    fn ReusePointGetPlan(
        &mut self,
        session: &CompilerSession,
        info_schema: &dyn CompilerInfoSchema,
        prepared: &mut plannercore::PlanCacheStmt,
        plan: &mut Box<dyn base::Plan>,
    ) -> Result<bool, errors::Error>;
    fn WarmUpTransaction(
        &mut self,
        session: &CompilerSession,
        plan: &dyn base::Plan,
    ) -> Result<(), errors::Error>;
    fn BuildExecStmt(&mut self, input: ExecStmtBuildInput) -> Box<ExecStmt>;
    fn RecoverCompilePanic(&self, panic_value: &(dyn Any + Send)) -> Option<errors::Error>;
    fn LogCompilePanic(&mut self, ctx: &CompilerContext, sql: &str, error: &errors::Error);
}

// Compiler：将 ast.StmtNode 编译为物理执行计划。
// Compiler compiles an ast.StmtNode to a physical plan.

/// SQL 编译器：持有会话与依赖注入。
pub struct Compiler {
    /// 编译会话。
    pub Ctx: CompilerSession,
    /// 外部依赖实现。
    pub dependencies: Box<dyn CompilerDependencies>,
}

impl Compiler {
    // Compile：编译一条语句；捕获 panic 并尝试恢复为错误。
    // Compile compiles an ast.StmtNode to a physical plan.
    /// 将 AST 编译为 `ExecStmt`；外层捕获编译 panic。
    pub fn Compile(
        &mut self,
        ctx: CompilerContext,
        statement: ast::NodeRef,
    ) -> Result<Box<ExecStmt>, errors::Error> {
        // 开启编译区域，保证结束时 End。
        let sql = self.dependencies.StatementText(&statement);
        let (region, ctx) = self.dependencies.StartCompileRegion(ctx);
        let _region_guard = CompileRegionGuard(Some(region));
        // 内层编译；panic 时尝试 Recover 为业务错误。
        let compiled = catch_unwind(AssertUnwindSafe(|| {
            self.compileInner(ctx.clone(), statement)
        }));
        match compiled {
            Ok(result) => result,
            Err(panic_value) => {
                if let Some(error) = self.dependencies.RecoverCompilePanic(panic_value.as_ref()) {
                    self.dependencies.LogCompilePanic(&ctx, &sql, &error);
                    Err(error)
                } else {
                    resume_unwind(panic_value)
                }
            }
        }
    }

    /// 编译主路径：预处理 → 优化 → 指标 → 优先级 → 追踪 → 构建。
    fn compileInner(
        &mut self,
        ctx: CompilerContext,
        statement: ast::NodeRef,
    ) -> Result<Box<ExecStmt>, errors::Error> {
        // 预处理并校验事务状态。
        self.dependencies
            .SetStatementReadOnly(&self.Ctx, &statement);
        let preprocessed = self
            .dependencies
            .Preprocess(ctx.clone(), &self.Ctx, &statement)?;
        self.dependencies
            .AssertTransactionState(&self.Ctx, &preprocessed);

        // 取事务信息模式、会话状态、可选 prepared 缓存。
        let info_schema = self.dependencies.TransactionInfoSchema(&self.Ctx);
        let session_state = self.dependencies.SessionState(&self.Ctx);
        let mut prepared = self.dependencies.PreparedStatement(
            &statement,
            &self.Ctx,
            &preprocessed.resolveContext,
        )?;
        // 优化生成计划。
        let mut optimized = self.dependencies.Optimize(
            ctx.clone(),
            &self.Ctx,
            &statement,
            &preprocessed.resolveContext,
            info_schema.as_ref(),
        )?;
        self.dependencies.AssertStatementStaleness(&self.Ctx);

        // 语句视图：优先用 prepared 缓存中的视图。
        let statement_view = prepared
            .as_ref()
            .map(|prepared| prepared.statementView.clone())
            .unwrap_or_else(|| {
                self.dependencies
                    .StatementView(&statement, &preprocessed.resolveContext)
            });
        CountStmtNode(
            ctx.clone(),
            statement_view,
            session_state.inRestrictedSQL,
            &session_state.resourceGroup,
            self.dependencies.as_mut(),
        );

        // 无显式优先级时，按估算行数决定是否降低优先级。
        let lower_priority = session_state.priority == astersql_parser_mysql::r#const::NoPriority.0
            && needLowerPriority(optimized.plan.as_ref(), self.dependencies.as_ref());
        self.dependencies
            .SetPlan(&self.Ctx, optimized.plan.as_ref());

        // 可选：输出计划追踪（digest + 脱敏归一化计划）。
        if let Some(plan_trace) = self.dependencies.PlanTrace(&ctx, &self.Ctx, &statement) {
            let digest_hex = plan_trace
                .digest
                .as_deref()
                .map(hexEncode)
                .unwrap_or_default();
            let normalized = (!plan_trace.normalizedPlan.is_empty()).then(|| {
                self.dependencies
                    .RedactNormalizedPlan(session_state.redactLog, &plan_trace.normalizedPlan)
            });
            self.dependencies.EmitPlanTrace(
                &ctx,
                &digest_hex,
                &plan_trace.statementType,
                session_state.connectionID,
                normalized.as_deref(),
            );
        }

        // 尝试复用 PointGet 计划缓存。
        let reused_prepared = if let Some(prepared_statement) = prepared.as_mut() {
            self.dependencies.ReusePointGetPlan(
                &self.Ctx,
                info_schema.as_ref(),
                prepared_statement.cache.as_mut(),
                &mut optimized.plan,
            )?
        } else {
            false
        };
        // 事务预热后构建可执行语句。
        self.dependencies
            .WarmUpTransaction(&self.Ctx, optimized.plan.as_ref())?;

        Ok(self.dependencies.BuildExecStmt(ExecStmtBuildInput {
            context: ctx,
            infoSchema: info_schema,
            plan: optimized.plan,
            lowerPriority: lower_priority,
            statement,
            session: self.Ctx.clone(),
            outputNames: optimized.outputNames,
            prepared: preparedCacheForExec(prepared, reused_prepared),
        }))
    }
}

/// Go only sets `ExecStmt.PsStmt` when the prepared PointGet executor is safe
/// to reuse. Keep an ordinary EXECUTE statement detached from that cache.
pub(crate) fn preparedCacheForExec(
    prepared: Option<CompilerPreparedStatement>,
    reused: bool,
) -> Option<Box<plannercore::PlanCacheStmt>> {
    reused
        .then(|| prepared.map(|prepared| prepared.cache))
        .flatten()
}

/// 将字节序列编码为小写十六进制字符串。
fn hexEncode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(DIGITS[(byte >> 4) as usize] as char);
        encoded.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    encoded
}

// 检查物理算子估算行数是否超过昂贵查询阈值，从而需要降低优先级。
// needLowerPriority checks whether any physical operator exceeds the configured
// expensive-query threshold.
/// 判断计划是否需要降低执行优先级。
pub fn needLowerPriority(plan: &dyn base::Plan, dependencies: &dyn CompilerDependencies) -> bool {
    match dependencies.PriorityPlan(plan) {
        PriorityPlan::Physical(plan) => {
            isPhysicalPlanNeedLowerPriority(&plan, dependencies.ExpensiveThreshold())
        }
        PriorityPlan::Execute(plan) => needLowerPriorityView(&plan, dependencies),
        PriorityPlan::Insert(plan) | PriorityPlan::Delete(plan) | PriorityPlan::Update(plan) => {
            plan.as_ref().is_some_and(|plan| {
                isPhysicalPlanNeedLowerPriority(plan, dependencies.ExpensiveThreshold())
            })
        }
        PriorityPlan::Other => false,
    }
}

/// 对 PriorityPlan 视图递归判定是否降低优先级。
fn needLowerPriorityView(plan: &PriorityPlan, dependencies: &dyn CompilerDependencies) -> bool {
    match plan {
        PriorityPlan::Physical(plan) => {
            isPhysicalPlanNeedLowerPriority(plan, dependencies.ExpensiveThreshold())
        }
        PriorityPlan::Execute(plan) => needLowerPriorityView(plan, dependencies),
        PriorityPlan::Insert(plan) | PriorityPlan::Delete(plan) | PriorityPlan::Update(plan) => {
            plan.as_ref().is_some_and(|plan| {
                isPhysicalPlanNeedLowerPriority(plan, dependencies.ExpensiveThreshold())
            })
        }
        PriorityPlan::Other => false,
    }
}

/// 物理计划或其任一子节点估算行数超过阈值则需要降低优先级。
pub fn isPhysicalPlanNeedLowerPriority(plan: &PhysicalPriorityPlan, threshold: i64) -> bool {
    plan.estimatedRows as i64 > threshold
        || plan
            .children
            .iter()
            .any(|child| isPhysicalPlanNeedLowerPriority(child, threshold))
}

// 按语句类型统计执行次数（非受限 SQL）。
// CountStmtNode records the number of statements with the same type.
/// 按语句类型与数据库标签递增指标计数。
pub fn CountStmtNode(
    _ctx: CompilerContext,
    statement: CompilerStatementView,
    in_restricted_sql: bool,
    resource_group: &str,
    dependencies: &mut dyn CompilerDependencies,
) {
    // 受限 SQL（内部）不计入用户侧指标。
    if in_restricted_sql {
        return;
    }
    let mode = dependencies.MetricMode();
    if mode.recordQPSByDB || mode.recordDBLabel {
        let databases = getStmtDbLabel(statement.clone());
        if mode.recordQPSByDB {
            for database in databases {
                dependencies.IncrementDBStatement(&database, &statement.statementType);
            }
        } else {
            for database in databases {
                dependencies.IncrementStatement(
                    &statement.statementType,
                    &database,
                    resource_group,
                );
            }
        }
    } else {
        dependencies.IncrementStatement(&statement.statementType, "", resource_group);
    }
}

/// 从语句视图收集涉及的数据库名集合；为空时放入空串占位。
pub fn getStmtDbLabel(statement: CompilerStatementView) -> HashSet<String> {
    let mut databases = HashSet::new();
    match statement.databases {
        StatementDatabaseView::DirectTable(database)
        | StatementDatabaseView::NonTransactional(database) => {
            if let Some(database) = database {
                databases.insert(database);
            }
        }
        StatementDatabaseView::Insert { table, select } => {
            databases.extend(getDbFromResultNode(table));
            databases.extend(getDbFromResultNode(select));
        }
        StatementDatabaseView::Rename(items) => {
            databases.extend(items.into_iter().flatten());
        }
        StatementDatabaseView::Tables(items) => {
            databases.extend(items);
        }
        StatementDatabaseView::Result(result) | StatementDatabaseView::UpdateOrDelete(result) => {
            databases.extend(getDbFromResultNode(result));
        }
        StatementDatabaseView::Call(database) => {
            if let Some(database) = database {
                databases.insert(database);
            }
        }
        StatementDatabaseView::Show { database, table } => {
            databases.insert(database);
            if let Some(database) = table {
                databases.insert(database);
            }
        }
        StatementDatabaseView::Use(database) => {
            databases.insert(database);
        }
        StatementDatabaseView::Binding { origin, hinted } => {
            databases.extend(getDbFromResultNode(origin));
            if databases.is_empty() {
                databases.extend(getDbFromResultNode(hinted));
            }
        }
        StatementDatabaseView::Other => {}
    }
    if databases.is_empty() {
        databases.insert(String::new());
    }
    databases
}

// 适配器在构造视图前已通过 resolve.Context 解析 TableName；此处有意保留重复库名。
// The adapter resolves TableName through resolve.Context before constructing
// this view. Duplicate database names are intentionally retained here.
/// 从结果节点树递归收集数据库名。
pub fn getDbFromResultNode(result: Option<ResultNodeView>) -> Vec<String> {
    let Some(result) = result else {
        return Vec::new();
    };
    match result {
        ResultNodeView::TableSource(source) => getDbFromResultNode(Some(*source)),
        ResultNodeView::Select(from) => getDbFromResultNode(from.map(|node| *node)),
        ResultNodeView::TableName(database) => database.into_iter().collect(),
        ResultNodeView::Join { left, right } => {
            let mut databases = getDbFromResultNode(left.map(|node| *node));
            databases.extend(getDbFromResultNode(right.map(|node| *node)));
            databases
        }
        ResultNodeView::Other => Vec::new(),
    }
}
