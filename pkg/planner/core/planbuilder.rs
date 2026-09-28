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
// Copyright 2026 AsterSQL.

// 计划构建器（PlanBuilder）核心：语句 → 执行计划。
//
// 将解析后的 Statement（SELECT/DDL/ADMIN/SHOW/ANALYZE 等）转为 BuiltPlan，
// 并收集权限访问信息、访问路径、统计任务与 Schema；与 Go planner/core
// 的 planbuilder 职责对齐。执行计划是优化器输出的算子树，供执行器解释执行。

use crate::find_best_task::{AccessPath, IndexInfo};
use crate::task::{Expression, FieldType, PlanKind, PlanNode, TypeCode};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq)]
/// 权限枚举：表级/动态权限等，用于 visitInfo 收集。
pub enum Privilege {
    Select,
    Insert,
    Update,
    Delete,
    Create,
    Drop,
    Alter,
    Grant,
    Reload,
    Super,
    Dynamic(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 权限访问信息：库表列、错误与动态权限标记。
pub struct visitInfo {
    /// 所需权限。
    pub privilege: Privilege,
    /// 数据库名。
    pub db: String,
    /// 表名。
    pub table: String,
    /// 列名。
    pub column: String,
    /// 关联错误信息。
    pub error: String,
    /// 是否允许写类 ALTER 权限语义。
    pub alterWritable: bool,
    /// 动态权限名列表。
    pub dynamicPrivs: Vec<String>,
    /// 动态权限是否带 GRANT OPTION。
    pub dynamicWithGrant: bool,
}
impl visitInfo {
    /// 比较两条 visitInfo 是否相等。
    pub fn Equals(&self, other: &Self) -> bool {
        self == other
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
/// SQL 子句位置编码，用于报错上下文。
pub enum clauseCode {
    #[default]
    unknowClause,
    fieldList,
    havingClause,
    onClause,
    orderByClause,
    whereClause,
    groupByClause,
    showStatement,
    globalOrderByClause,
    expressionClause,
    windowOrderByClause,
    partitionByClause,
}
/// 将 clauseCode 转为可读子句描述。
pub fn clauseMsg(code: clauseCode) -> &'static str {
    match code {
        clauseCode::unknowClause => "",
        clauseCode::fieldList => "field list",
        clauseCode::havingClause => "having clause",
        clauseCode::onClause => "on clause",
        clauseCode::orderByClause => "order clause",
        clauseCode::whereClause => "where clause",
        clauseCode::groupByClause => "group statement",
        clauseCode::showStatement => "show statement",
        clauseCode::globalOrderByClause => "global ORDER clause",
        clauseCode::expressionClause => "expression",
        clauseCode::windowOrderByClause => "window order by",
        clauseCode::partitionByClause => "window partition by",
    }
}
/// 计划构建能力标志位类型。
pub type capFlagType = u64;
/// 能力：允许扩展/改写 AST。
pub const canExpandAST: capFlagType = 1;
/// 能力：允许视图重命名相关处理。
pub const renameView: capFlagType = 2;
/// 子查询上下文标志类型。
pub type subQueryCtx = u64;
/// 非子查询上下文。
pub const subQueryCtxNone: subQueryCtx = 0;
/// EXISTS 子查询上下文。
pub const subQueryCtxExists: subQueryCtx = 1;
/// IN 子查询上下文。
pub const subQueryCtxIn: subQueryCtx = 2;

#[derive(Clone, Debug, Default)]
/// CTE 构建状态：递归性、是否在建、是否进入子查询。
pub struct cteInfo {
    /// 名称。
    pub name: String,
    /// 是否非递归 CTE。
    pub nonRecursive: bool,
    /// 是否使用递归。
    pub useRecursive: bool,
    /// 是否正在构建该 CTE。
    pub isBuilding: bool,
    /// 是否已进入子查询。
    pub enterSubquery: bool,
}

#[derive(Clone, Debug, Default)]
/// 按作用域栈管理表 handle 列下标映射。
pub struct handleColHelper {
    scopes: Vec<HashMap<i64, Vec<usize>>>,
}
impl handleColHelper {
    /// 清空状态以便 PlanBuilder 对象池复用。
    pub fn resetForReuse(&mut self) {
        self.scopes.clear();
    }
    /// 弹出当前 handle 列作用域映射。
    pub fn popMap(&mut self) -> HashMap<i64, Vec<usize>> {
        self.scopes.pop().expect("handle-column scope must exist")
    }
    /// 压入一层 handle 列作用域映射。
    pub fn pushMap(&mut self, map: HashMap<i64, Vec<usize>>) {
        self.scopes.push(map);
    }
    /// 合并左右子树 handle 映射后压栈。
    pub fn mergeAndPush(
        &mut self,
        mut left: HashMap<i64, Vec<usize>>,
        right: HashMap<i64, Vec<usize>>,
    ) {
        for (id, handles) in right {
            left.entry(id).or_default().extend(handles);
        }
        self.pushMap(left);
    }
    /// 查看栈顶 handle 映射（不弹出）。
    pub fn tailMap(&self) -> Option<&HashMap<i64, Vec<usize>>> {
        self.scopes.last()
    }
}

#[derive(Clone, Debug)]
/// 列元数据：ID、名、偏移、类型与主键/生成列标记。
pub struct ColumnInfo {
    /// 标识 ID。
    pub id: i64,
    /// 名称。
    pub name: String,
    /// 列偏移。
    pub offset: usize,
    /// 字段类型。
    pub field_type: FieldType,
    /// 是否生成列。
    pub generated: bool,
    /// 生成列是否 STORED。
    pub stored: bool,
    /// 是否隐藏列。
    pub hidden: bool,
    /// 是否主键列。
    pub primary_key: bool,
}
#[derive(Clone, Debug)]
/// 表元数据：列、索引、分区与 handle 形态。
pub struct TableInfo {
    /// 标识 ID。
    pub id: i64,
    /// 数据库名。
    pub db: String,
    /// 名称。
    pub name: String,
    /// 列列表。
    pub columns: Vec<ColumnInfo>,
    /// 索引列表。
    pub indices: Vec<IndexMeta>,
    /// 分区列表。
    pub partitions: Vec<PartitionInfo>,
    /// 是否使用 common handle（聚簇索引非 int）。
    pub common_handle: bool,
    /// 整型主键是否即 handle。
    pub pk_is_handle: bool,
    /// 是否临时表。
    pub temporary: bool,
}
#[derive(Clone, Debug)]
/// 索引元数据：列下标、前缀长度、唯一/全局/向量等。
pub struct IndexMeta {
    /// 标识 ID。
    pub id: i64,
    /// 名称。
    pub name: String,
    /// 列列表。
    pub columns: Vec<usize>,
    /// 前缀索引长度。
    pub prefix_lengths: Vec<Option<usize>>,
    /// 是否唯一索引。
    pub unique: bool,
    /// 是否全局索引。
    pub global: bool,
    /// 是否不可见索引。
    pub invisible: bool,
    /// 是否多值索引。
    pub multi_valued: bool,
    /// 是否向量索引。
    pub vector: bool,
}
#[derive(Clone, Debug)]
/// 分区元数据：分区 ID 与名称。
pub struct PartitionInfo {
    /// 标识 ID。
    pub id: i64,
    /// 名称。
    pub name: String,
}

#[derive(Clone, Debug)]
/// 结果集 Schema 中的一列（名、类型、标志）。
pub struct SchemaColumn {
    /// 名称。
    pub name: String,
    /// 字段类型。
    pub field_type: FieldType,
    /// 列类型标志位。
    pub flag: u32,
}
/// 结果集 Schema：列描述向量。
pub type Schema = Vec<SchemaColumn>;

#[derive(Clone, Debug)]
/// 字面量/用户变量/DEFAULT 等常量值枚举。
pub enum Value {
    Null,
    Int(i64),
    UInt(u64),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
    UserVar(String),
    Default,
}

#[derive(Clone, Debug)]
/// 计划构建器可处理的语句形态枚举。
pub enum Statement {
    Select {
        plan: PlanNode,
        for_update: bool,
    },
    Execute {
        name: String,
        using: Vec<Value>,
    },
    Do(Vec<Expression>),
    Set(Vec<(String, Value)>),
    SetConfig {
        component: String,
        name: String,
        value: Value,
    },
    CreateBinding {
        sql: String,
        hinted_sql: String,
        plan_digest: Option<String>,
    },
    DropBinding {
        sql: String,
    },
    SetBindingStatus {
        sql: String,
        enabled: bool,
    },
    Prepare {
        name: String,
        sql: String,
    },
    Admin(AdminStatement),
    Analyze(AnalyzeStatement),
    Show(ShowKind),
    Insert(InsertStatement),
    LoadData {
        table: TableInfo,
        local: bool,
    },
    ImportInto {
        table: TableInfo,
        assignments: Vec<(String, Expression)>,
    },
    LoadStats {
        path: String,
    },
    RefreshStats(Vec<(String, String)>),
    LockStats(Vec<(String, String)>),
    UnlockStats(Vec<(String, String)>),
    DistributeTable {
        table: TableInfo,
        rule: String,
        engine: String,
    },
    SplitRegion {
        table: TableInfo,
        index: Option<String>,
        values: Vec<Vec<Value>>,
    },
    Ddl {
        kind: String,
        table: Option<TableInfo>,
    },
    Trace {
        format: String,
        stmt: Box<Statement>,
    },
    Explain {
        format: String,
        analyze: bool,
        explore: bool,
        stmt: Box<Statement>,
    },
    SelectInto {
        source: Box<Statement>,
        target: String,
    },
    PlanReplayer {
        capture: bool,
        ts: Option<Value>,
    },
    Traffic {
        action: String,
    },
    CompactTable(TableInfo),
    RecommendIndex {
        table: TableInfo,
    },
    AlterDdlJob {
        job_ids: Vec<i64>,
        options: Vec<AlterDDLJobOpt>,
    },
    Simple(String),
}

#[derive(Clone, Debug)]
/// ADMIN 子语句种类。
pub enum AdminStatement {
    CheckTable(TableInfo),
    CheckIndex(TableInfo, String),
    ShowDdl,
    ShowDdlJobs,
    CancelDdlJobs(Vec<i64>),
    PauseDdlJobs(Vec<i64>),
    ResumeDdlJobs(Vec<i64>),
    RecoverIndex(TableInfo, String),
    CleanupIndex(TableInfo, String),
    AlterDdlJob(Vec<i64>, Vec<AlterDDLJobOpt>),
}
#[derive(Clone, Debug)]
/// SHOW 语句种类（表、警告、Region 等）。
pub enum ShowKind {
    Tables,
    Databases,
    Columns,
    Index,
    Warnings,
    Errors,
    Slow,
    Regions,
    Distribution,
    BackupMeta,
    BackupQuery,
    TrafficJobs,
    Triggers,
    Events,
    ProcedureStatus,
    NextRowId,
}
#[derive(Clone, Debug)]
/// INSERT/REPLACE 语句输入结构。
pub struct InsertStatement {
    /// 表名。
    pub table: TableInfo,
    /// 列列表。
    pub columns: Vec<String>,
    pub values: Vec<Vec<Value>>,
    pub select: Option<Box<Statement>>,
    pub on_duplicate: Vec<(String, Expression)>,
    pub replace: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
/// ANALYZE 选项键：Buckets、TopN、CMSketch 等。
pub enum AnalyzeOptionType {
    Buckets,
    TopN,
    SampleNum,
    SampleRate,
    CmsketchDepth,
    CmsketchWidth,
    NumSamples,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// ANALYZE 列选择：全部/谓词列/指定列。
pub enum ColumnChoice {
    Default,
    All,
    Predicate,
    List,
}
#[derive(Clone, Debug)]
/// ANALYZE TABLE 语句描述。
pub struct AnalyzeStatement {
    /// 表名。
    pub table: TableInfo,
    pub partition_names: Vec<String>,
    pub index_names: Vec<String>,
    /// 列列表。
    pub columns: Vec<String>,
    pub column_choice: ColumnChoice,
    /// 选项列表。
    pub options: Vec<(AnalyzeOptionType, u64)>,
    pub version: i32,
    pub incremental: bool,
}
#[derive(Clone, Debug)]
/// 单个索引统计任务。
pub struct AnalyzeIndexTask {
    /// 选用的索引元数据。
    pub index: IndexMeta,
    pub physical_id: i64,
    pub partition_name: String,
    pub version: i32,
}
#[derive(Clone, Debug)]
/// 单组列统计任务。
pub struct AnalyzeColumnsTask {
    /// 列列表。
    pub columns: Vec<ColumnInfo>,
    pub physical_id: i64,
    pub partition_name: String,
    /// 选项列表。
    pub options: HashMap<AnalyzeOptionType, u64>,
    pub version: i32,
}

#[derive(Clone, Debug)]
/// PlanBuilder 输出的已构建计划变体。
pub enum BuiltPlan {
    Logical(PlanNode),
    Execute {
        name: String,
        using: Vec<Value>,
    },
    Set(Vec<(String, Value)>),
    Binding {
        operation: String,
        sql: String,
        hinted_sql: String,
    },
    Prepare {
        name: String,
        sql: String,
    },
    Admin {
        kind: String,
        schema: Schema,
        payload: Vec<String>,
    },
    Analyze {
        index_tasks: Vec<AnalyzeIndexTask>,
        column_tasks: Vec<AnalyzeColumnsTask>,
    },
    Show {
        kind: ShowKind,
        schema: Schema,
    },
    Insert {
        table: TableInfo,
        columns: Vec<ColumnInfo>,
        rows: Vec<Vec<Value>>,
        generated: Vec<(ColumnInfo, Expression)>,
        on_duplicate: Vec<(String, Expression)>,
        replace: bool,
    },
    Ddl {
        kind: String,
        table: Option<TableInfo>,
    },
    Explain {
        target: Box<BuiltPlan>,
        format: String,
        analyze: bool,
        explore: bool,
    },
    Trace {
        target: Box<BuiltPlan>,
        format: String,
    },
    Command {
        name: String,
        schema: Schema,
        arguments: Vec<String>,
    },
}

#[derive(Clone, Debug)]
/// 计划构建错误包装。
pub struct BuilderError(pub String);
/// 预处理与计划构建共用的 Result 别名。
pub type Result<T> = std::result::Result<T, BuilderError>;

/// PlanBuilder 构造选项 trait。
pub trait PlanBuilderOpt {
    /// 将选项应用到 PlanBuilder。
    fn Apply(&self, builder: &mut PlanBuilder);
}
/// 选项：禁止执行侧副作用的构建模式。
pub struct PlanBuilderOptNoExecution;
impl PlanBuilderOpt for PlanBuilderOptNoExecution {
    /// 将选项应用到 PlanBuilder。
    fn Apply(&self, builder: &mut PlanBuilder) {
        builder.disableSubQueryPreprocessing = true;
    }
}
/// 选项：允许 CAST ARRAY 相关路径。
pub struct PlanBuilderOptAllowCastArray;
impl PlanBuilderOpt for PlanBuilderOptAllowCastArray {
    /// 将选项应用到 PlanBuilder。
    fn Apply(&self, builder: &mut PlanBuilder) {
        builder.allowBuildCastArray = true;
    }
}

#[derive(Default)]
/// 核心计划构建器：由语句生成逻辑/管理计划。
pub struct PlanBuilder {
    pub visitInfo: Vec<visitInfo>,
    pub optFlag: u64,
    pub curClause: clauseCode,
    pub qbOffset: Vec<i32>,
    pub handleHelper: handleColHelper,
    pub outerCTEs: Vec<cteInfo>,
    pub disableSubQueryPreprocessing: bool,
    pub allowBuildCastArray: bool,
    pub enableSemiJoinRewrite: bool,
    pub noDecorrelate: bool,
    pub isForUpdateRead: bool,
    pub warnings: Vec<String>,
    pub unusedViewHints: Vec<String>,
    nonViableFTSMatch: bool,
    predicateMatchSeen: bool,
}

/// 使用选项列表构造 PlanBuilder。
pub fn NewPlanBuilder(opts: &[&dyn PlanBuilderOpt]) -> PlanBuilder {
    let mut builder = PlanBuilder::default();
    for option in opts {
        option.Apply(&mut builder);
    }
    builder
}
/// 返回构建器相关的初始化字符串表（指标/标签）。
pub fn init() -> (&'static [&'static str], &'static [&'static str]) {
    (
        &["leader-scatter", "peer-scatter", "learner-scatter"],
        &["tikv", "tiflash"],
    )
}

impl PlanBuilder {
    /// 完成 PlanBuilder 初始化并返回自身。
    pub fn Init(mut self) -> Self {
        self.ResetForReuse();
        self
    }
    /// 重置ForReuse（对应同名 Go 逻辑）。
    pub fn ResetForReuse(&mut self) -> &mut Self {
        self.visitInfo.clear();
        self.optFlag = 0;
        self.curClause = clauseCode::unknowClause;
        self.qbOffset.clear();
        self.handleHelper.resetForReuse();
        self.outerCTEs.clear();
        self.isForUpdateRead = false;
        self.warnings.clear();
        self.nonViableFTSMatch = false;
        self.predicateMatchSeen = false;
        self
    }
    /// 是否标记了不可行的全文匹配。
    pub fn HasNonViableFTSMatch(&self) -> bool {
        self.nonViableFTSMatch
    }
    /// 标记存在不可行的全文匹配。
    pub fn MarkNonViableFTSMatch(&mut self) {
        self.nonViableFTSMatch = true;
    }
    /// 是否检测到谓词匹配提示命中。
    pub fn HasPredicateMatch(&self) -> bool {
        self.predicateMatchSeen
    }
    /// 标记谓词匹配已发生。
    pub fn MarkPredicateMatch(&mut self) {
        self.predicateMatchSeen = true;
    }
    /// 取出权限访问信息列表。
    pub fn GetVisitInfo(&self) -> &[visitInfo] {
        &self.visitInfo
    }
    /// 是否为 FOR UPDATE 读路径。
    pub fn GetIsForUpdateRead(&self) -> bool {
        self.isForUpdateRead
    }
    /// 取出优化器提示构建状态。
    pub fn GetHintState(&self) -> &[String] {
        &self.unusedViewHints
    }
    /// 取出优化标志位。
    pub fn GetOptFlag(&self) -> u64 {
        self.optFlag
    }
    /// 当前 SELECT 查询块偏移。
    pub fn getSelectOffset(&self) -> i32 {
        self.qbOffset.last().copied().unwrap_or(-1)
    }
    /// 压入查询块偏移。
    pub fn pushSelectOffset(&mut self, offset: i32) {
        self.qbOffset.push(offset);
    }
    /// 弹出查询块偏移。
    pub fn popSelectOffset(&mut self) {
        self.qbOffset.pop().expect("select offset scope must exist");
    }
    /// 处理未使用的视图提示。
    pub fn HandleUnusedViewHints(&mut self) {
        for hint in self.unusedViewHints.drain(..) {
            self.warnings.push(format!("unused view hint: {hint}"));
        }
    }
    /// 记录构建器指标并返回计数。
    pub fn recordPlanBuilderMetric(&self) -> usize {
        self.visitInfo.len() + self.warnings.len()
    }

    /// 根据语句分派并构建对应计划。
    pub fn Build(&mut self, statement: &Statement) -> Result<BuiltPlan> {
        match statement {
            Statement::Select { plan, for_update } => {
                self.isForUpdateRead = *for_update;
                Ok(BuiltPlan::Logical(plan.clone()))
            }
            Statement::Execute { name, using } => self.buildExecute(name, using),
            Statement::Do(exprs) => self.buildDo(exprs),
            Statement::Set(values) => self.buildSet(values),
            Statement::SetConfig {
                component,
                name,
                value,
            } => self.buildSetConfig(component, name, value),
            Statement::CreateBinding {
                sql,
                hinted_sql,
                plan_digest,
            } => {
                if let Some(digest) = plan_digest {
                    self.buildCreateBindPlanFromPlanDigest(sql, hinted_sql, digest)
                } else {
                    self.buildCreateBindPlan(sql, hinted_sql)
                }
            }
            Statement::DropBinding { sql } => self.buildDropBindPlan(sql),
            Statement::SetBindingStatus { sql, enabled } => {
                self.buildSetBindingStatusPlan(sql, *enabled)
            }
            Statement::Prepare { name, sql } => Ok(self.buildPrepare(name, sql)),
            Statement::Admin(admin) => self.buildAdmin(admin),
            Statement::Analyze(analyze) => self.buildAnalyze(analyze),
            Statement::Show(show) => self.buildShow(show),
            Statement::Insert(insert) => self.buildInsert(insert),
            Statement::LoadData { table, local } => self.buildLoadData(table, *local),
            Statement::ImportInto { table, assignments } => {
                self.buildImportInto(table, assignments)
            }
            Statement::LoadStats { path } => Ok(self.buildLoadStats(path)),
            Statement::RefreshStats(objects) => self.buildRefreshStats(objects),
            Statement::LockStats(objects) => Ok(self.buildLockStats(objects)),
            Statement::UnlockStats(objects) => Ok(self.buildUnlockStats(objects)),
            Statement::DistributeTable {
                table,
                rule,
                engine,
            } => self.buildDistributeTable(table, rule, engine),
            Statement::SplitRegion {
                table,
                index,
                values,
            } => self.buildSplitRegion(table, index.as_deref(), values),
            Statement::Ddl { kind, table } => self.buildDDL(kind, table.as_ref()),
            Statement::Trace { format, stmt } => self.buildTrace(format, stmt),
            Statement::Explain {
                format,
                analyze,
                explore,
                stmt,
            } => self.buildExplain(format, *analyze, *explore, stmt),
            Statement::SelectInto { source, target } => self.buildSelectInto(source, target),
            Statement::PlanReplayer { capture, ts } => {
                Ok(self.buildPlanReplayer(*capture, ts.as_ref()))
            }
            Statement::Traffic { action } => Ok(self.buildTraffic(action)),
            Statement::CompactTable(table) => self.buildCompactTable(table),
            Statement::RecommendIndex { table } => self.buildRecommendIndex(table),
            Statement::AlterDdlJob { job_ids, options } => {
                self.buildAdminAlterDDLJob(job_ids, options)
            }
            Statement::Simple(name) => self.buildSimple(name),
        }
    }

    /// 构建SetConfig（对应同名 Go 逻辑）。
    pub fn buildSetConfig(
        &mut self,
        component: &str,
        name: &str,
        value: &Value,
    ) -> Result<BuiltPlan> {
        if component.is_empty() || name.is_empty() {
            return Err(BuilderError(
                "component and variable name are required".into(),
            ));
        }
        Ok(BuiltPlan::Set(vec![(
            format!("{component}.{name}"),
            value.clone(),
        )]))
    }
    /// 构建Execute（对应同名 Go 逻辑）。
    pub fn buildExecute(&self, name: &str, using: &[Value]) -> Result<BuiltPlan> {
        if name.is_empty() {
            return Err(BuilderError("prepared statement name is empty".into()));
        }
        Ok(BuiltPlan::Execute {
            name: name.into(),
            using: using.to_vec(),
        })
    }
    /// 构建Do（对应同名 Go 逻辑）。
    pub fn buildDo(&self, exprs: &[Expression]) -> Result<BuiltPlan> {
        let mut plan = PlanNode::new(PlanKind::Projection);
        plan.expressions = exprs.to_vec();
        plan.stats.row_count = 1.0;
        Ok(BuiltPlan::Logical(plan))
    }
    /// 构建Set（对应同名 Go 逻辑）。
    pub fn buildSet(&self, values: &[(String, Value)]) -> Result<BuiltPlan> {
        if values.iter().any(|(name, _)| name.is_empty()) {
            return Err(BuilderError("SET variable name is empty".into()));
        }
        Ok(BuiltPlan::Set(values.to_vec()))
    }
    /// 构建DropBindPlan（对应同名 Go 逻辑）。
    pub fn buildDropBindPlan(&mut self, sql: &str) -> Result<BuiltPlan> {
        checkHintedSQL(sql, "utf8mb4", "utf8mb4_bin", "")?;
        Ok(BuiltPlan::Binding {
            operation: "drop".into(),
            sql: sql.into(),
            hinted_sql: String::new(),
        })
    }
    /// 构建SetBindingStatusPlan（对应同名 Go 逻辑）。
    pub fn buildSetBindingStatusPlan(&mut self, sql: &str, enabled: bool) -> Result<BuiltPlan> {
        checkHintedSQL(sql, "utf8mb4", "utf8mb4_bin", "")?;
        Ok(BuiltPlan::Binding {
            operation: if enabled { "enable" } else { "disable" }.into(),
            sql: sql.into(),
            hinted_sql: String::new(),
        })
    }
    /// 构建CreateBindPlan（对应同名 Go 逻辑）。
    pub fn buildCreateBindPlan(&mut self, sql: &str, hinted: &str) -> Result<BuiltPlan> {
        checkHintedSQL(hinted, "utf8mb4", "utf8mb4_bin", "")?;
        if normalize_sql(sql) != normalize_sql(hinted) {
            return Err(BuilderError(
                "binding SQL and hinted SQL differ structurally".into(),
            ));
        }
        Ok(BuiltPlan::Binding {
            operation: "create".into(),
            sql: sql.into(),
            hinted_sql: hinted.into(),
        })
    }
    /// 构建CreateBindPlanFromPlanDigest（对应同名 Go 逻辑）。
    pub fn buildCreateBindPlanFromPlanDigest(
        &mut self,
        sql: &str,
        hinted: &str,
        digest: &str,
    ) -> Result<BuiltPlan> {
        if digest.is_empty() {
            return Err(BuilderError("plan digest is empty".into()));
        }
        self.buildCreateBindPlan(sql, hinted)
    }
    /// 构建Prepare（对应同名 Go 逻辑）。
    pub fn buildPrepare(&self, name: &str, sql: &str) -> BuiltPlan {
        BuiltPlan::Prepare {
            name: name.into(),
            sql: sql.into(),
        }
    }

    /// 构建Admin（对应同名 Go 逻辑）。
    pub fn buildAdmin(&mut self, admin: &AdminStatement) -> Result<BuiltPlan> {
        match admin {
            AdminStatement::CheckTable(table) => self.buildAdminCheckTable(table),
            AdminStatement::CheckIndex(table, index) => {
                let (schema, _) = self.buildCheckIndexSchema(table, index)?;
                Ok(BuiltPlan::Admin {
                    kind: "check-index".into(),
                    schema,
                    payload: vec![table.name.clone(), index.clone()],
                })
            }
            AdminStatement::ShowDdl => Ok(BuiltPlan::Admin {
                kind: "show-ddl".into(),
                schema: buildShowDDLFields().0,
                payload: Vec::new(),
            }),
            AdminStatement::ShowDdlJobs => Ok(BuiltPlan::Admin {
                kind: "show-ddl-jobs".into(),
                schema: buildShowDDLJobsFields().0,
                payload: Vec::new(),
            }),
            AdminStatement::CancelDdlJobs(ids)
            | AdminStatement::PauseDdlJobs(ids)
            | AdminStatement::ResumeDdlJobs(ids) => Ok(BuiltPlan::Admin {
                kind: format!("{:?}", admin),
                schema: buildCommandOnDDLJobsFields().0,
                payload: ids.iter().map(ToString::to_string).collect(),
            }),
            AdminStatement::RecoverIndex(t, i) | AdminStatement::CleanupIndex(t, i) => {
                Ok(BuiltPlan::Admin {
                    kind: format!("{:?}", admin),
                    schema: buildRecoverIndexFields().0,
                    payload: vec![t.name.clone(), i.clone()],
                })
            }
            AdminStatement::AlterDdlJob(ids, options) => self.buildAdminAlterDDLJob(ids, options),
        }
    }
    /// 构建AdminCheckTable（对应同名 Go 逻辑）。
    pub fn buildAdminCheckTable(&mut self, table: &TableInfo) -> Result<BuiltPlan> {
        let indices: Vec<_> = table
            .indices
            .iter()
            .filter(|i| !i.invisible)
            .map(|i| i.name.clone())
            .collect();
        Ok(BuiltPlan::Admin {
            kind: "check-table".into(),
            schema: Vec::new(),
            payload: indices,
        })
    }
    /// 构建CheckIndexSchema（对应同名 Go 逻辑）。
    pub fn buildCheckIndexSchema(
        &self,
        table: &TableInfo,
        index: &str,
    ) -> Result<(Schema, Vec<String>)> {
        let index = table
            .indices
            .iter()
            .find(|i| i.name.eq_ignore_ascii_case(index))
            .ok_or_else(|| BuilderError("index does not exist".into()))?;
        let columns = getIndexColumnInfos(table, index);
        let schema = columns
            .iter()
            .map(|c| SchemaColumn {
                name: c.name.clone(),
                field_type: c.field_type.clone(),
                flag: 0,
            })
            .collect();
        let names = columns.iter().map(|c| c.name.clone()).collect();
        Ok((schema, names))
    }

    /// 构建Analyze（对应同名 Go 逻辑）。
    pub fn buildAnalyze(&mut self, analyze: &AnalyzeStatement) -> Result<BuiltPlan> {
        let opts = handleAnalyzeOptions(&analyze.options)?;
        if analyze.index_names.is_empty() {
            self.buildAnalyzeTable(analyze, opts)
        } else {
            self.buildAnalyzeIndex(analyze, opts)
        }
    }
    /// 构建AnalyzeTable（对应同名 Go 逻辑）。
    pub fn buildAnalyzeTable(
        &mut self,
        analyze: &AnalyzeStatement,
        options: HashMap<AnalyzeOptionType, u64>,
    ) -> Result<BuiltPlan> {
        let ids = GetPhysicalIDsAndPartitionNames(&analyze.table, &analyze.partition_names)?;
        let columns = getAnalyzeColumnList(&analyze.columns, &analyze.table)?;
        let tasks = ids
            .into_iter()
            .map(|(id, name)| AnalyzeColumnsTask {
                columns: columns.clone(),
                physical_id: id,
                partition_name: name,
                options: options.clone(),
                version: analyze.version,
            })
            .collect();
        Ok(BuiltPlan::Analyze {
            index_tasks: Vec::new(),
            column_tasks: tasks,
        })
    }
    /// 构建AnalyzeIndex（对应同名 Go 逻辑）。
    pub fn buildAnalyzeIndex(
        &mut self,
        analyze: &AnalyzeStatement,
        _options: HashMap<AnalyzeOptionType, u64>,
    ) -> Result<BuiltPlan> {
        let ids = GetPhysicalIDsAndPartitionNames(&analyze.table, &analyze.partition_names)?;
        let mut tasks = Vec::new();
        for name in &analyze.index_names {
            let index = analyze
                .table
                .indices
                .iter()
                .find(|i| i.name.eq_ignore_ascii_case(name))
                .ok_or_else(|| BuilderError(format!("index {name} does not exist")))?;
            tasks.extend(generateIndexTasks(index, &ids, analyze.version));
        }
        Ok(BuiltPlan::Analyze {
            index_tasks: tasks,
            column_tasks: Vec::new(),
        })
    }
    /// 构建AnalyzeAllIndex（对应同名 Go 逻辑）。
    pub fn buildAnalyzeAllIndex(
        &mut self,
        analyze: &AnalyzeStatement,
        _options: HashMap<AnalyzeOptionType, u64>,
    ) -> Result<BuiltPlan> {
        let ids = GetPhysicalIDsAndPartitionNames(&analyze.table, &analyze.partition_names)?;
        let tasks = analyze
            .table
            .indices
            .iter()
            .flat_map(|i| generateIndexTasks(i, &ids, analyze.version))
            .collect();
        Ok(BuiltPlan::Analyze {
            index_tasks: tasks,
            column_tasks: Vec::new(),
        })
    }

    /// 构建Show（对应同名 Go 逻辑）。
    pub fn buildShow(&self, show: &ShowKind) -> Result<BuiltPlan> {
        Ok(BuiltPlan::Show {
            kind: show.clone(),
            schema: buildShowSchema(show),
        })
    }
    /// 构建Simple（对应同名 Go 逻辑）。
    pub fn buildSimple(&self, name: &str) -> Result<BuiltPlan> {
        Ok(BuiltPlan::Command {
            name: name.into(),
            schema: Vec::new(),
            arguments: Vec::new(),
        })
    }
    /// 构建 INSERT/REPLACE 计划。
    pub fn buildInsert(&mut self, insert: &InsertStatement) -> Result<BuiltPlan> {
        let columns = self.getAffectCols(insert)?;
        let rows = self.buildValuesListOfInsert(insert, &columns)?;
        let generated =
            self.resolveGeneratedColumns(&insert.table.columns, &insert.on_duplicate)?;
        Ok(BuiltPlan::Insert {
            table: insert.table.clone(),
            columns,
            rows,
            generated,
            on_duplicate: insert.on_duplicate.clone(),
            replace: insert.replace,
        })
    }
    /// 获取AffectCols（对应同名 Go 逻辑）。
    pub fn getAffectCols(&self, insert: &InsertStatement) -> Result<Vec<ColumnInfo>> {
        if insert.columns.is_empty() {
            let visible = insert
                .table
                .columns
                .iter()
                .filter(|column| !column.hidden)
                .cloned()
                .collect::<Vec<_>>();
            let supplies_generated_defaults =
                insert.values.iter().all(|row| row.len() == visible.len());
            if supplies_generated_defaults {
                return Ok(visible);
            }
            return Ok(visible
                .into_iter()
                .filter(|column| !column.generated)
                .collect());
        }
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        for name in &insert.columns {
            if !seen.insert(name.to_lowercase()) {
                return Err(BuilderError(format!("column {name} specified twice")));
            }
            result.push(
                insert
                    .table
                    .columns
                    .iter()
                    .find(|c| c.name.eq_ignore_ascii_case(name))
                    .cloned()
                    .ok_or_else(|| BuilderError(format!("unknown column {name}")))?,
            );
        }
        Ok(result)
    }
    /// 构建ValuesListOfInsert（对应同名 Go 逻辑）。
    pub fn buildValuesListOfInsert(
        &self,
        insert: &InsertStatement,
        columns: &[ColumnInfo],
    ) -> Result<Vec<Vec<Value>>> {
        let mut out = Vec::new();
        for row in &insert.values {
            if row.len() != columns.len() {
                return Err(BuilderError(
                    "column count does not match value count".into(),
                ));
            }
            out.push(
                row.iter()
                    .zip(columns)
                    .map(|(v, c)| {
                        if c.generated && !matches!(v, Value::Default) {
                            return Err(BuilderError(format!(
                                "the value specified for generated column {} is not allowed",
                                c.name
                            )));
                        }
                        convertValue(v, c)
                    })
                    .collect::<Result<Vec<_>>>()?,
            );
        }
        Ok(out)
    }
    /// 解析GeneratedColumns（对应同名 Go 逻辑）。
    pub fn resolveGeneratedColumns(
        &self,
        columns: &[ColumnInfo],
        on_duplicate: &[(String, Expression)],
    ) -> Result<Vec<(ColumnInfo, Expression)>> {
        let updated: HashSet<_> = on_duplicate.iter().map(|(n, _)| n.to_lowercase()).collect();
        Ok(columns
            .iter()
            .filter(|c| c.generated && !updated.contains(&c.name.to_lowercase()))
            .cloned()
            .map(|c| {
                let expr = Expression {
                    name: format!("generated:{}", c.name),
                    return_type: Some(c.field_type.clone()),
                    ..Expression::default()
                };
                (c, expr)
            })
            .collect())
    }
    /// 构建LoadData（对应同名 Go 逻辑）。
    pub fn buildLoadData(&mut self, table: &TableInfo, local: bool) -> Result<BuiltPlan> {
        self.requireInsertAndSelectPriv(std::slice::from_ref(table));
        Ok(BuiltPlan::Command {
            name: "load-data".into(),
            schema: Vec::new(),
            arguments: vec![table.name.clone(), local.to_string()],
        })
    }
    /// 构建ImportInto（对应同名 Go 逻辑）。
    pub fn buildImportInto(
        &mut self,
        table: &TableInfo,
        assignments: &[(String, Expression)],
    ) -> Result<BuiltPlan> {
        checkImportIntoColAssignments(assignments)?;
        self.requireInsertAndSelectPriv(std::slice::from_ref(table));
        Ok(BuiltPlan::Command {
            name: "import-into".into(),
            schema: Vec::new(),
            arguments: vec![table.name.clone()],
        })
    }
    /// 构建LoadStats（对应同名 Go 逻辑）。
    pub fn buildLoadStats(&self, path: &str) -> BuiltPlan {
        BuiltPlan::Command {
            name: "load-stats".into(),
            schema: Vec::new(),
            arguments: vec![path.into()],
        }
    }
    /// 构建RefreshStats（对应同名 Go 逻辑）。
    pub fn buildRefreshStats(&mut self, objects: &[(String, String)]) -> Result<BuiltPlan> {
        let objects = fillDefaultDBForStatsObjects(objects, "test")?;
        self.requireSelectOrRestoreAdminPrivForStatsObjects(&objects);
        Ok(BuiltPlan::Command {
            name: "refresh-stats".into(),
            schema: Vec::new(),
            arguments: objects
                .into_iter()
                .map(|(d, t)| format!("{d}.{t}"))
                .collect(),
        })
    }
    /// 构建LockStats（对应同名 Go 逻辑）。
    pub fn buildLockStats(&mut self, objects: &[(String, String)]) -> BuiltPlan {
        self.requireSelectPrivForStatsObjects(objects);
        BuiltPlan::Command {
            name: "lock-stats".into(),
            schema: Vec::new(),
            arguments: objects.iter().map(|(d, t)| format!("{d}.{t}")).collect(),
        }
    }
    /// 构建UnlockStats（对应同名 Go 逻辑）。
    pub fn buildUnlockStats(&mut self, objects: &[(String, String)]) -> BuiltPlan {
        self.requireSelectPrivForStatsObjects(objects);
        BuiltPlan::Command {
            name: "unlock-stats".into(),
            schema: Vec::new(),
            arguments: objects.iter().map(|(d, t)| format!("{d}.{t}")).collect(),
        }
    }
    /// 要求SelectOrRestoreAdminPrivForStatsObjects（对应同名 Go 逻辑）。
    pub fn requireSelectOrRestoreAdminPrivForStatsObjects(&mut self, objects: &[(String, String)]) {
        for (db, table) in objects {
            self.visitInfo.push(visitInfo {
                privilege: Privilege::Select,
                db: db.clone(),
                table: table.clone(),
                column: String::new(),
                error: String::new(),
                alterWritable: false,
                dynamicPrivs: vec!["RESTORE_ADMIN".into()],
                dynamicWithGrant: false,
            });
        }
    }
    /// 要求SelectPrivForStatsObjects（对应同名 Go 逻辑）。
    pub fn requireSelectPrivForStatsObjects(&mut self, objects: &[(String, String)]) {
        for (db, table) in objects {
            self.visitInfo.push(visitInfo {
                privilege: Privilege::Select,
                db: db.clone(),
                table: table.clone(),
                column: String::new(),
                error: String::new(),
                alterWritable: false,
                dynamicPrivs: Vec::new(),
                dynamicWithGrant: false,
            });
        }
    }
    /// 要求InsertAndSelectPriv（对应同名 Go 逻辑）。
    pub fn requireInsertAndSelectPriv(&mut self, tables: &[TableInfo]) {
        for table in tables {
            for privilege in [Privilege::Insert, Privilege::Select] {
                self.visitInfo.push(visitInfo {
                    privilege,
                    db: table.db.clone(),
                    table: table.name.clone(),
                    column: String::new(),
                    error: String::new(),
                    alterWritable: false,
                    dynamicPrivs: Vec::new(),
                    dynamicWithGrant: false,
                });
            }
        }
    }
    /// 构建DistributeTable（对应同名 Go 逻辑）。
    pub fn buildDistributeTable(
        &self,
        table: &TableInfo,
        rule: &str,
        engine: &str,
    ) -> Result<BuiltPlan> {
        if !["leader-scatter", "peer-scatter", "learner-scatter"].contains(&rule)
            || !["tikv", "tiflash"].contains(&engine)
        {
            return Err(BuilderError("invalid distribution rule or engine".into()));
        }
        Ok(BuiltPlan::Command {
            name: "distribute-table".into(),
            schema: buildDistributeTableSchema().0,
            arguments: vec![table.name.clone(), rule.into(), engine.into()],
        })
    }
    /// 构建SplitRegion（对应同名 Go 逻辑）。
    pub fn buildSplitRegion(
        &self,
        table: &TableInfo,
        index: Option<&str>,
        values: &[Vec<Value>],
    ) -> Result<BuiltPlan> {
        if let Some(index) = index {
            self.buildSplitIndexRegion(table, index, values)
        } else {
            self.buildSplitTableRegion(table, values)
        }
    }
    /// 构建SplitIndexRegion（对应同名 Go 逻辑）。
    pub fn buildSplitIndexRegion(
        &self,
        table: &TableInfo,
        index: &str,
        values: &[Vec<Value>],
    ) -> Result<BuiltPlan> {
        let idx = table
            .indices
            .iter()
            .find(|i| i.name.eq_ignore_ascii_case(index))
            .ok_or_else(|| BuilderError("index does not exist".into()))?;
        for (row, values) in values.iter().enumerate() {
            convertValueListToData(values, &getIndexColumnInfos(table, idx), row)?;
        }
        Ok(BuiltPlan::Command {
            name: "split-index-region".into(),
            schema: buildSplitRegionsSchema().0,
            arguments: vec![table.name.clone(), index.into()],
        })
    }
    /// 构建SplitTableRegion（对应同名 Go 逻辑）。
    pub fn buildSplitTableRegion(
        &self,
        table: &TableInfo,
        values: &[Vec<Value>],
    ) -> Result<BuiltPlan> {
        let handles = buildHandleColumnInfos(table);
        for (row, values) in values.iter().enumerate() {
            convertValueListToData(values, &handles, row)?;
        }
        Ok(BuiltPlan::Command {
            name: "split-table-region".into(),
            schema: buildSplitRegionsSchema().0,
            arguments: vec![table.name.clone()],
        })
    }
    /// 构建DDL（对应同名 Go 逻辑）。
    pub fn buildDDL(&mut self, kind: &str, table: Option<&TableInfo>) -> Result<BuiltPlan> {
        self.checkSEMStmt(kind)?;
        if let Some(table) = table {
            self.visitInfo.push(visitInfo {
                privilege: Privilege::Alter,
                db: table.db.clone(),
                table: table.name.clone(),
                column: String::new(),
                error: String::new(),
                alterWritable: true,
                dynamicPrivs: Vec::new(),
                dynamicWithGrant: false,
            });
        }
        Ok(BuiltPlan::Ddl {
            kind: kind.into(),
            table: table.cloned(),
        })
    }
    /// 构建Trace（对应同名 Go 逻辑）。
    pub fn buildTrace(&mut self, format: &str, stmt: &Statement) -> Result<BuiltPlan> {
        let target = self.Build(stmt)?;
        Ok(BuiltPlan::Trace {
            target: Box::new(target),
            format: format.into(),
        })
    }
    /// 构建 EXPLAIN（含 analyze/format）计划。
    pub fn buildExplain(
        &mut self,
        format: &str,
        analyze: bool,
        explore: bool,
        stmt: &Statement,
    ) -> Result<BuiltPlan> {
        let target = self.Build(stmt)?;
        self.buildExplainPlan(target, format, analyze, explore)
    }
    /// 构建ExplainPlan（对应同名 Go 逻辑）。
    pub fn buildExplainPlan(
        &self,
        target: BuiltPlan,
        format: &str,
        analyze: bool,
        explore: bool,
    ) -> Result<BuiltPlan> {
        let supported = [
            "row",
            "brief",
            "verbose",
            "dot",
            "hint",
            "json",
            "cost_trace",
        ];
        if !supported.contains(&format) {
            return Err(BuilderError(format!("unsupported explain format {format}")));
        }
        Ok(BuiltPlan::Explain {
            target: Box::new(target),
            format: format.into(),
            analyze,
            explore,
        })
    }
    /// 构建SelectInto（对应同名 Go 逻辑）。
    pub fn buildSelectInto(&mut self, source: &Statement, target: &str) -> Result<BuiltPlan> {
        let _ = self.Build(source)?;
        Ok(BuiltPlan::Command {
            name: "select-into".into(),
            schema: Vec::new(),
            arguments: vec![target.into()],
        })
    }
    /// 构建PlanReplayer（对应同名 Go 逻辑）。
    pub fn buildPlanReplayer(&self, capture: bool, ts: Option<&Value>) -> BuiltPlan {
        BuiltPlan::Command {
            name: "plan-replayer".into(),
            schema: Vec::new(),
            arguments: vec![capture.to_string(), calcTSForPlanReplayer(ts).to_string()],
        }
    }
    /// 构建Traffic（对应同名 Go 逻辑）。
    pub fn buildTraffic(&self, action: &str) -> BuiltPlan {
        BuiltPlan::Command {
            name: "traffic".into(),
            schema: Vec::new(),
            arguments: vec![action.into()],
        }
    }
    /// 构建CompactTable（对应同名 Go 逻辑）。
    pub fn buildCompactTable(&self, table: &TableInfo) -> Result<BuiltPlan> {
        if table.temporary {
            return Err(BuilderError("temporary table cannot be compacted".into()));
        }
        Ok(BuiltPlan::Command {
            name: "compact-table".into(),
            schema: Vec::new(),
            arguments: vec![table.name.clone()],
        })
    }
    /// 构建RecommendIndex（对应同名 Go 逻辑）。
    pub fn buildRecommendIndex(&self, table: &TableInfo) -> Result<BuiltPlan> {
        Ok(BuiltPlan::Command {
            name: "recommend-index".into(),
            schema: buildAddQueryWatchSchema().0,
            arguments: vec![table.name.clone()],
        })
    }
    /// 构建AdminAlterDDLJob（对应同名 Go 逻辑）。
    pub fn buildAdminAlterDDLJob(
        &self,
        ids: &[i64],
        options: &[AlterDDLJobOpt],
    ) -> Result<BuiltPlan> {
        for option in options {
            checkAlterDDLJobOptValue(option)?;
        }
        Ok(BuiltPlan::Admin {
            kind: "alter-ddl-job".into(),
            schema: buildCommandOnDDLJobsFields().0,
            payload: ids.iter().map(ToString::to_string).collect(),
        })
    }
    /// 校验SEMStmt（对应同名 Go 逻辑）。
    pub fn checkSEMStmt(&self, statement: &str) -> Result<()> {
        if statement.to_ascii_lowercase().contains("restricted") {
            Err(BuilderError("statement is unavailable in SEM mode".into()))
        } else {
            Ok(())
        }
    }

    /// 检测AggInExprNode（对应同名 Go 逻辑）。
    pub fn detectAggInExprNode(&self, exprs: &[Expression]) -> bool {
        exprs.iter().any(|e| {
            e.name.starts_with("agg:")
                || matches!(
                    e.name.to_ascii_lowercase().as_str(),
                    "sum" | "count" | "avg" | "min" | "max"
                )
        })
    }
    /// 检测SelectAgg（对应同名 Go 逻辑）。
    pub fn detectSelectAgg(&self, plan: &PlanNode) -> bool {
        !plan.agg_funcs.is_empty()
            || self.detectAggInExprNode(&plan.expressions)
            || plan.children.iter().any(|p| self.detectSelectAgg(p))
    }
    /// 检测SelectWindow（对应同名 Go 逻辑）。
    pub fn detectSelectWindow(&self, plan: &PlanNode) -> bool {
        plan.kind == PlanKind::Window
            || plan
                .expressions
                .iter()
                .any(|e| e.name.starts_with("window:"))
            || plan.children.iter().any(|p| self.detectSelectWindow(p))
    }
    /// 构建SelectLock（对应同名 Go 逻辑）。
    pub fn buildSelectLock(&mut self, mut source: PlanNode, lock: &str) -> Result<PlanNode> {
        if !isForUpdateReadSelectLock(Some(lock)) {
            return Err(BuilderError("unsupported select lock".into()));
        }
        self.isForUpdateRead = true;
        let mut plan = PlanNode::new(PlanKind::Other("Lock".into()));
        plan.labels.insert(
            "for_update".into(),
            if lock.eq_ignore_ascii_case("for update") {
                1.0
            } else {
                0.0
            },
        );
        plan.schema = std::mem::take(&mut source.schema);
        plan.children = vec![source];
        Ok(plan)
    }
    /// 构建PhysicalIndexLookUpReader（对应同名 Go 逻辑）。
    pub fn buildPhysicalIndexLookUpReader(
        &self,
        table: &TableInfo,
        index: &IndexMeta,
    ) -> Result<BuiltPlan> {
        if !checkIndexLookUpPushDownSupported(table, index, false) {
            return Err(BuilderError("index lookup cannot be pushed down".into()));
        }
        let mut index_scan = PlanNode::new(PlanKind::IndexScan);
        index_scan.schema = getIndexColsSchema(table, index);
        index_scan.ranges = 1;
        let mut table_scan = PlanNode::new(PlanKind::TableScan);
        table_scan.schema = table.columns.iter().map(|c| c.field_type.clone()).collect();
        let mut reader = PlanNode::new(PlanKind::IndexLookupReader);
        reader.children = vec![index_scan, table_scan];
        Ok(BuiltPlan::Logical(reader))
    }
    /// 构建PhysicalIndexLookUpReaders（对应同名 Go 逻辑）。
    pub fn buildPhysicalIndexLookUpReaders(
        &self,
        table: &TableInfo,
        indices: &[IndexMeta],
    ) -> Result<(Vec<BuiltPlan>, Vec<IndexMeta>)> {
        let mut plans = Vec::new();
        let mut used = Vec::new();
        for index in indices {
            if checkIndexLookUpPushDownSupported(table, index, true) {
                plans.push(self.buildPhysicalIndexLookUpReader(table, index)?);
                used.push(index.clone());
            }
        }
        Ok((plans, used))
    }
    /// 获取ColsInfo（对应同名 Go 逻辑）。
    pub fn getColsInfo(&self, table: &TableInfo) -> (Vec<IndexMeta>, Vec<ColumnInfo>) {
        (table.indices.clone(), table.columns.clone())
    }
    /// 获取MustAnalyzedColumns（对应同名 Go 逻辑）。
    pub fn getMustAnalyzedColumns(
        &self,
        table: &TableInfo,
        cache: &mut calcOnceMap,
    ) -> HashSet<i64> {
        cache.get_or_calculate(|| {
            let mut ids: HashSet<_> = table
                .columns
                .iter()
                .filter(|c| c.primary_key || c.generated)
                .map(|c| c.id)
                .collect();
            addColumnsWithVirtualExprs(table, &mut ids);
            ids
        })
    }
    /// 获取PredicateColumns（对应同名 Go 逻辑）。
    pub fn getPredicateColumns(&self, table: &TableInfo, cache: &mut calcOnceMap) -> HashSet<i64> {
        cache.get_or_calculate(|| {
            table
                .indices
                .iter()
                .flat_map(|i| {
                    i.columns
                        .iter()
                        .filter_map(|offset| table.columns.get(*offset).map(|c| c.id))
                })
                .collect()
        })
    }
    /// 获取FullAnalyzeColumnsInfo（对应同名 Go 逻辑）。
    pub fn getFullAnalyzeColumnsInfo(
        &self,
        table: &TableInfo,
        specified: &[String],
        choice: ColumnChoice,
    ) -> Result<(Vec<ColumnInfo>, ColumnChoice)> {
        let columns = match choice {
            ColumnChoice::All => table
                .columns
                .iter()
                .filter(|c| !c.hidden)
                .cloned()
                .collect(),
            ColumnChoice::List => getAnalyzeColumnList(specified, table)?,
            ColumnChoice::Predicate => {
                let ids: HashSet<_> = table
                    .indices
                    .iter()
                    .flat_map(|i| {
                        i.columns
                            .iter()
                            .filter_map(|offset| table.columns.get(*offset).map(|c| c.id))
                    })
                    .collect();
                getColumnListFromSet(&table.columns, &ids)
            }
            ColumnChoice::Default => getAnalyzeColumnList(specified, table)?,
        };
        Ok((columns, choice))
    }
    /// 获取ColumnsBasedOnPredicateColumns（对应同名 Go 逻辑）。
    pub fn getColumnsBasedOnPredicateColumns(
        &self,
        table: &TableInfo,
        predicate: &HashSet<i64>,
        must: &HashSet<i64>,
    ) -> Vec<ColumnInfo> {
        let ids = combineColumnSets(&[predicate.clone(), must.clone()]);
        getColumnListFromSet(&table.columns, &ids)
    }
    /// 获取ModifiedIndexesInfoForAnalyze（对应同名 Go 逻辑）。
    pub fn getModifiedIndexesInfoForAnalyze(
        &self,
        table: &TableInfo,
        columns: &HashSet<i64>,
    ) -> Vec<IndexMeta> {
        table
            .indices
            .iter()
            .filter(|index| {
                index.columns.iter().any(|offset| {
                    table
                        .columns
                        .get(*offset)
                        .is_some_and(|c| columns.contains(&c.id))
                })
            })
            .cloned()
            .collect()
    }
    /// 构建AnalyzeFullSamplingTask（对应同名 Go 逻辑）。
    pub fn buildAnalyzeFullSamplingTask(
        &self,
        _table: &TableInfo,
        columns: Vec<ColumnInfo>,
        physical_id: i64,
        partition: String,
        options: HashMap<AnalyzeOptionType, u64>,
        version: i32,
    ) -> AnalyzeColumnsTask {
        AnalyzeColumnsTask {
            columns,
            physical_id,
            partition_name: partition,
            options,
            version,
        }
    }
    /// 生成V2AnalyzeOptions（对应同名 Go 逻辑）。
    pub fn genV2AnalyzeOptions(
        &self,
        statement: &[(AnalyzeOptionType, u64)],
        saved: &HashMap<AnalyzeOptionType, u64>,
    ) -> Result<HashMap<AnalyzeOptionType, u64>> {
        Ok(mergeAnalyzeOptions(handleAnalyzeOptions(statement)?, saved))
    }
    /// 获取SavedAnalyzeOpts（对应同名 Go 逻辑）。
    pub fn getSavedAnalyzeOpts(
        &self,
        saved: &HashMap<
            i64,
            (
                HashMap<AnalyzeOptionType, u64>,
                ColumnChoice,
                Vec<ColumnInfo>,
            ),
        >,
        physical_id: i64,
    ) -> (
        HashMap<AnalyzeOptionType, u64>,
        ColumnChoice,
        Vec<ColumnInfo>,
    ) {
        saved
            .get(&physical_id)
            .cloned()
            .unwrap_or_else(|| (HashMap::new(), ColumnChoice::Default, Vec::new()))
    }
    /// analyzeVersionMatchesForPhysicalIDs：计划构建相关符号（对齐 Go 同名定义）。
    pub fn analyzeVersionMatchesForPhysicalIDs(
        &self,
        versions: &HashMap<i64, i32>,
        ids: &[i64],
        requested: i32,
    ) -> bool {
        ids.iter()
            .all(|id| versions.get(id).is_none_or(|v| *v == requested))
    }
    /// 追加AnalyzeVersionOverwriteWarning（对应同名 Go 逻辑）。
    pub fn appendAnalyzeVersionOverwriteWarning(&mut self) {
        self.warnings
            .push("analyze version is overwritten to match existing statistics".into());
    }
    /// 获取DefaultValueForInsert（对应同名 Go 逻辑）。
    pub fn getDefaultValueForInsert(&self, column: &ColumnInfo) -> Result<Value> {
        if column.generated {
            return Err(BuilderError(format!(
                "generated column {} has no insert default",
                column.name
            )));
        }
        Ok(match column.field_type.code {
            TypeCode::Null => Value::Null,
            TypeCode::Int => Value::Int(0),
            TypeCode::UInt => Value::UInt(0),
            TypeCode::Float | TypeCode::Decimal => Value::Float(0.0),
            TypeCode::String => Value::String(String::new()),
            TypeCode::Bytes | TypeCode::Vector => Value::Bytes(Vec::new()),
        })
    }
    /// 获取InsertColExpr（对应同名 Go 逻辑）。
    pub fn getInsertColExpr(&self, column: &ColumnInfo, value: &Value) -> Result<Expression> {
        let value = convertValue(value, column)?;
        Ok(Expression {
            name: format!("literal:{value:?}"),
            return_type: Some(column.field_type.clone()),
            ..Expression::default()
        })
    }
    /// 构建SelectPlanOfInsert（对应同名 Go 逻辑）。
    pub fn buildSelectPlanOfInsert(
        &mut self,
        insert: &InsertStatement,
    ) -> Result<Option<BuiltPlan>> {
        match insert.select.as_deref() {
            Some(statement) => self.Build(statement).map(Some),
            None => Ok(None),
        }
    }
    /// convertValue2ColumnType：计划构建相关符号（对齐 Go 同名定义）。
    pub fn convertValue2ColumnType(
        &self,
        values: &[Value],
        index: &IndexMeta,
        table: &TableInfo,
    ) -> Result<Vec<Value>> {
        convertValueListToData(values, &getIndexColumnInfos(table, index), 0)
    }
    /// 构建ExplainFor（对应同名 Go 逻辑）。
    pub fn buildExplainFor(&self, target: BuiltPlan, format: &str) -> Result<BuiltPlan> {
        self.buildExplainPlan(target, format, false, false)
    }
}

/// 设置ExtraPhysTblIDColsOnDataSource（对应同名 Go 逻辑）。
pub fn setExtraPhysTblIDColsOnDataSource(plan: &mut PlanNode, table_columns: &HashMap<i64, usize>) {
    for (table, column) in table_columns {
        plan.labels
            .insert(format!("physical_table_id:{table}"), *column as f64);
    }
    for child in &mut plan.children {
        setExtraPhysTblIDColsOnDataSource(child, table_columns);
    }
}
/// 校验IsAllSpecialGlobalIndex（对应同名 Go 逻辑）。
pub fn checkIsAllSpecialGlobalIndex(table: &TableInfo, names: &[String]) -> Result<bool> {
    for name in names {
        if !table
            .indices
            .iter()
            .any(|i| i.name.eq_ignore_ascii_case(name))
        {
            return Err(BuilderError(format!("index {name} does not exist")));
        }
    }
    Ok(!names.is_empty()
        && names.iter().all(|name| {
            table.indices.iter().any(|i| {
                i.name.eq_ignore_ascii_case(name) && i.global && (i.vector || i.multi_valued)
            })
        }))
}
/// 获取HintedStmtThroughPlanDigest（对应同名 Go 逻辑）。
pub fn getHintedStmtThroughPlanDigest(
    records: &HashMap<String, (String, String, String, String, String)>,
    digest: &str,
) -> Result<String> {
    let (_, sql, hint, _, _) = fetchRecordFromClusterStmtSummary(records, digest)?;
    Ok(format!("{hint} {sql}"))
}

/// colNameInOnDupExtractor：计划构建相关符号（对齐 Go 同名定义）。
pub struct colNameInOnDupExtractor {
    /// 输出列名。
    pub names: HashSet<String>,
}
impl colNameInOnDupExtractor {
    /// 进入节点时的预处理钩子（对应 AST visitor Enter）。
    pub fn Enter(&mut self, expression: &Expression) -> bool {
        if let Some(column) = expression.column {
            self.names.insert(column.to_string());
        }
        true
    }
    /// 离开节点时的预处理钩子（对应 AST visitor Leave）。
    pub fn Leave(&mut self, _expression: &Expression) -> bool {
        true
    }
}
/// importIntoCollAssignmentChecker：计划构建相关符号（对齐 Go 同名定义）。
pub struct importIntoCollAssignmentChecker {
    /// 输出列名。
    pub names: HashSet<String>,
    pub duplicate: Option<String>,
}
/// 新建ImportIntoCollAssignmentChecker（对应同名 Go 逻辑）。
pub fn newImportIntoCollAssignmentChecker() -> importIntoCollAssignmentChecker {
    importIntoCollAssignmentChecker {
        names: HashSet::new(),
        duplicate: None,
    }
}
impl importIntoCollAssignmentChecker {
    /// 进入节点时的预处理钩子（对应 AST visitor Enter）。
    pub fn Enter(&mut self, name: &str) -> bool {
        if !self.names.insert(name.to_lowercase()) {
            self.duplicate = Some(name.into());
            return false;
        }
        true
    }
    /// 离开节点时的预处理钩子（对应 AST visitor Leave）。
    pub fn Leave(&self, _name: &str) -> bool {
        self.duplicate.is_none()
    }
}
/// userVariableChecker：计划构建相关符号（对齐 Go 同名定义）。
pub struct userVariableChecker {
    pub found: bool,
}
impl userVariableChecker {
    /// 进入节点时的预处理钩子（对应 AST visitor Enter）。
    pub fn Enter(&mut self, value: &Value) -> bool {
        if matches!(value, Value::UserVar(_)) {
            self.found = true;
        }
        !self.found
    }
    /// 离开节点时的预处理钩子（对应 AST visitor Leave）。
    pub fn Leave(&self, _value: &Value) -> bool {
        !self.found
    }
}

/// 获取DBTableInfo（对应同名 Go 逻辑）。
pub fn GetDBTableInfo(visits: &[visitInfo]) -> Vec<(String, String)> {
    let mut seen = HashSet::new();
    visits
        .iter()
        .filter_map(|v| {
            let pair = (v.db.clone(), v.table.clone());
            if v.table.is_empty() || !seen.insert(pair.clone()) {
                None
            } else {
                Some(pair)
            }
        })
        .collect()
}
/// 规范化_sql（对应同名 Go 逻辑）。
fn normalize_sql(sql: &str) -> String {
    sql.split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
        .replace(" /*+", "")
}
/// 校验HintedSQL（对应同名 Go 逻辑）。
pub fn checkHintedSQL(sql: &str, charset: &str, collation: &str, _db: &str) -> Result<()> {
    if sql.trim().is_empty() {
        return Err(BuilderError("SQL is empty".into()));
    }
    if charset.is_empty() || collation.is_empty() {
        return Err(BuilderError("charset and collation are required".into()));
    }
    if !sql
        .trim_start()
        .to_ascii_lowercase()
        .starts_with(|c: char| c.is_ascii_alphabetic())
    {
        return Err(BuilderError("invalid SQL".into()));
    }
    Ok(())
}
/// 拉取RecordFromClusterStmtSummary（对应同名 Go 逻辑）。
pub fn fetchRecordFromClusterStmtSummary(
    records: &HashMap<String, (String, String, String, String, String)>,
    digest: &str,
) -> Result<(String, String, String, String, String)> {
    records
        .get(digest)
        .cloned()
        .ok_or_else(|| BuilderError("plan digest not found in statement summary".into()))
}
/// 收集StrOrUserVarList（对应同名 Go 逻辑）。
pub fn collectStrOrUserVarList(
    list: &[Value],
    vars: &HashMap<String, String>,
) -> Result<Vec<String>> {
    list.iter()
        .map(|v| match v {
            Value::String(s) => Ok(s.clone()),
            Value::UserVar(name) => vars
                .get(name)
                .cloned()
                .ok_or_else(|| BuilderError(format!("user variable {name} is unset"))),
            _ => Err(BuilderError("string or user variable required".into())),
        })
        .collect()
}
/// 构造SQLBindOPFromPlanDigest（对应同名 Go 逻辑）。
pub fn constructSQLBindOPFromPlanDigest(
    records: &HashMap<String, (String, String, String, String, String)>,
    digest: &str,
) -> Result<(String, String)> {
    let (_, sql, hint, _, _) = fetchRecordFromClusterStmtSummary(records, digest)?;
    Ok((sql.clone(), format!("{} {}", hint, sql)))
}
/// 获取PathByIndexName（对应同名 Go 逻辑）。
pub fn getPathByIndexName<'a>(
    paths: &'a [AccessPath],
    name: &str,
    table: &TableInfo,
) -> Option<&'a AccessPath> {
    if isPrimaryIndex(name) && table.pk_is_handle {
        paths.iter().find(|p| p.is_int_handle)
    } else {
        let index = table
            .indices
            .iter()
            .find(|i| i.name.eq_ignore_ascii_case(name))?;
        paths.iter().find(|p| {
            p.index
                .as_ref()
                .is_some_and(|pi| pi.columns == index.columns)
        })
    }
}
/// 判断是否PrimaryIndex（对应同名 Go 逻辑）。
pub fn isPrimaryIndex(name: &str) -> bool {
    name.eq_ignore_ascii_case("primary")
}
/// 生成TiFlashPath（对应同名 Go 逻辑）。
pub fn genTiFlashPath(_table: &TableInfo) -> AccessPath {
    AccessPath {
        store: Some(crate::task::StoreType::TiFlash),
        is_single_scan: true,
        ..AccessPath::default()
    }
}
/// 填充ContentForTablePath（对应同名 Go 逻辑）。
pub fn fillContentForTablePath(path: &mut AccessPath, table: &TableInfo) {
    path.is_int_handle = table.pk_is_handle;
    path.is_common_handle = table.common_handle;
    path.is_single_scan = true;
}
/// 判断是否ForUpdateReadSelectLock（对应同名 Go 逻辑）。
pub fn isForUpdateReadSelectLock(lock: Option<&str>) -> bool {
    lock.is_some_and(|l| {
        matches!(
            l.to_ascii_lowercase().as_str(),
            "for update" | "for share" | "lock in share mode"
        )
    })
}
/// 判断是否TiKVIndexByName（对应同名 Go 逻辑）。
pub fn isTiKVIndexByName(name: &str, index: &IndexMeta, _table: &TableInfo) -> bool {
    index.name.eq_ignore_ascii_case(name) && !index.vector
}
/// 校验IndexLookUpPushDownSupported（对应同名 Go 逻辑）。
pub fn checkIndexLookUpPushDownSupported(
    table: &TableInfo,
    index: &IndexMeta,
    _suppress_warning: bool,
) -> bool {
    !table.temporary
        && !index.multi_valued
        && index.columns.iter().all(|offset| {
            table
                .columns
                .get(*offset)
                .is_some_and(|c| !matches!(c.field_type.code, TypeCode::Vector))
        })
}
/// 校验AutoForceIndexLookUpPushDown（对应同名 Go 逻辑）。
pub fn checkAutoForceIndexLookUpPushDown(table: &TableInfo, index: &IndexMeta) -> bool {
    checkIndexLookUpPushDownSupported(table, index, true) && index.unique
}
/// 获取PossibleAccessPaths（对应同名 Go 逻辑）。
pub fn getPossibleAccessPaths(
    table: &TableInfo,
    use_indices: &[String],
    ignore_indices: &[String],
    force_indices: &[String],
    has_tiflash: bool,
) -> Result<Vec<AccessPath>> {
    let mut table_path = AccessPath::default();
    fillContentForTablePath(&mut table_path, table);
    let mut paths = vec![table_path];
    for index in &table.indices {
        if index.invisible
            || ignore_indices
                .iter()
                .any(|n| index.name.eq_ignore_ascii_case(n))
        {
            continue;
        }
        if !use_indices.is_empty()
            && !use_indices
                .iter()
                .any(|n| index.name.eq_ignore_ascii_case(n))
            && !force_indices
                .iter()
                .any(|n| index.name.eq_ignore_ascii_case(n))
        {
            continue;
        }
        paths.push(AccessPath {
            index: Some(IndexInfo {
                columns: index.columns.clone(),
                prefix_lengths: index.prefix_lengths.clone(),
                unique: index.unique,
                global: index.global,
                multi_valued: index.multi_valued,
                vector: index.vector,
            }),
            index_columns: index.columns.clone(),
            is_single_scan: index.columns.len() == table.columns.len(),
            ..AccessPath::default()
        });
    }
    if has_tiflash {
        paths.push(genTiFlashPath(table));
    }
    if !force_indices.is_empty() {
        paths.retain(|p| {
            p.index.as_ref().is_some_and(|i| {
                force_indices.iter().any(|n| {
                    table
                        .indices
                        .iter()
                        .any(|m| m.name.eq_ignore_ascii_case(n) && m.columns == i.columns)
                })
            })
        });
    }
    if paths.is_empty() {
        return Err(BuilderError("no usable access path".into()));
    }
    Ok(paths)
}
/// 移除IgnoredPaths（对应同名 Go 逻辑）。
pub fn removeIgnoredPaths(mut paths: Vec<AccessPath>, ignored: &[AccessPath]) -> Vec<AccessPath> {
    paths.retain(|p| {
        !ignored
            .iter()
            .any(|i| i.index.as_ref().map(|x| &x.columns) == p.index.as_ref().map(|x| &x.columns))
    });
    paths
}
/// 移除GlobalIndexPaths（对应同名 Go 逻辑）。
pub fn removeGlobalIndexPaths(mut paths: Vec<AccessPath>) -> Vec<AccessPath> {
    paths.retain(|p| !p.index.as_ref().is_some_and(|i| i.global));
    paths
}
/// 获取IndexColumnInfos（对应同名 Go 逻辑）。
pub fn getIndexColumnInfos(table: &TableInfo, index: &IndexMeta) -> Vec<ColumnInfo> {
    index
        .columns
        .iter()
        .filter_map(|offset| table.columns.get(*offset).cloned())
        .collect()
}
/// 获取IndexColsSchema（对应同名 Go 逻辑）。
pub fn getIndexColsSchema(table: &TableInfo, index: &IndexMeta) -> Vec<FieldType> {
    getIndexColumnInfos(table, index)
        .into_iter()
        .map(|c| c.field_type)
        .collect()
}
/// 获取PhysicalID（对应同名 Go 逻辑）。
pub fn getPhysicalID(table: &TableInfo, global: bool) -> (i64, bool) {
    if global || table.partitions.is_empty() {
        (table.id, false)
    } else {
        (table.partitions[0].id, true)
    }
}
/// 尝试GetPkExtraColumn（对应同名 Go 逻辑）。
pub fn tryGetPkExtraColumn(table: &TableInfo) -> Option<ColumnInfo> {
    (!table.pk_is_handle && !table.common_handle).then(|| ColumnInfo {
        id: -1,
        name: "_tidb_rowid".into(),
        offset: table.columns.len(),
        field_type: int_type(),
        generated: false,
        stored: true,
        hidden: true,
        primary_key: true,
    })
}
/// 尝试GetCommonHandleCols（对应同名 Go 逻辑）。
pub fn tryGetCommonHandleCols(table: &TableInfo) -> Option<Vec<ColumnInfo>> {
    table.common_handle.then(|| {
        table
            .columns
            .iter()
            .filter(|c| c.primary_key)
            .cloned()
            .collect()
    })
}
/// 尝试GetPkHandleCol（对应同名 Go 逻辑）。
pub fn tryGetPkHandleCol(table: &TableInfo) -> Option<ColumnInfo> {
    table
        .pk_is_handle
        .then(|| table.columns.iter().find(|c| c.primary_key).cloned())
        .flatten()
}
/// 构建HandleColsForAnalyze（对应同名 Go 逻辑）。
pub fn BuildHandleColsForAnalyze(table: &TableInfo) -> Vec<ColumnInfo> {
    tryGetCommonHandleCols(table)
        .or_else(|| tryGetPkHandleCol(table).map(|c| vec![c]))
        .or_else(|| tryGetPkExtraColumn(table).map(|c| vec![c]))
        .unwrap_or_default()
}
/// 获取PhysicalIDsAndPartitionNames（对应同名 Go 逻辑）。
pub fn GetPhysicalIDsAndPartitionNames(
    table: &TableInfo,
    requested: &[String],
) -> Result<Vec<(i64, String)>> {
    if table.partitions.is_empty() {
        return Ok(vec![(table.id, String::new())]);
    }
    if requested.is_empty() {
        return Ok(table
            .partitions
            .iter()
            .map(|p| (p.id, p.name.clone()))
            .collect());
    }
    requested
        .iter()
        .map(|name| {
            table
                .partitions
                .iter()
                .find(|p| p.name.eq_ignore_ascii_case(name))
                .map(|p| (p.id, p.name.clone()))
                .ok_or_else(|| BuilderError(format!("unknown partition {name}")))
        })
        .collect()
}

#[derive(Default)]
/// calcOnceMap：计划构建相关符号（对齐 Go 同名定义）。
pub struct calcOnceMap {
    calculated: bool,
    columns: HashSet<i64>,
}
impl calcOnceMap {
    /// 获取_or_calculate（对应同名 Go 逻辑）。
    pub fn get_or_calculate<F: FnOnce() -> HashSet<i64>>(&mut self, calculate: F) -> HashSet<i64> {
        if !self.calculated {
            self.columns = calculate();
            self.calculated = true;
        }
        self.columns.clone()
    }
}
/// 添加ColumnsWithVirtualExprs（对应同名 Go 逻辑）。
pub fn addColumnsWithVirtualExprs(table: &TableInfo, ids: &mut HashSet<i64>) {
    let generated: Vec<_> = table
        .columns
        .iter()
        .filter(|c| c.generated && ids.contains(&c.id))
        .map(|c| c.offset)
        .collect();
    for offset in generated {
        for dependency in table.columns.iter().take(offset) {
            ids.insert(dependency.id);
        }
    }
}
/// 获取AnalyzeColumnList（对应同名 Go 逻辑）。
pub fn getAnalyzeColumnList(specified: &[String], table: &TableInfo) -> Result<Vec<ColumnInfo>> {
    if specified.is_empty() {
        return Ok(table
            .columns
            .iter()
            .filter(|c| !c.hidden)
            .cloned()
            .collect());
    }
    specified
        .iter()
        .map(|name| {
            table
                .columns
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(name))
                .cloned()
                .ok_or_else(|| BuilderError(format!("unknown column {name}")))
        })
        .collect()
}
/// 合并ColumnSets（对应同名 Go 逻辑）。
pub fn combineColumnSets(sets: &[HashSet<i64>]) -> HashSet<i64> {
    sets.iter().flat_map(|s| s.iter().copied()).collect()
}
/// 获取ColumnSetFromSpecifiedCols（对应同名 Go 逻辑）。
pub fn getColumnSetFromSpecifiedCols(cols: &[ColumnInfo]) -> HashSet<i64> {
    cols.iter().map(|c| c.id).collect()
}
/// 获取MissingColumns（对应同名 Go 逻辑）。
pub fn getMissingColumns(columns: &HashSet<i64>, required: &HashSet<i64>) -> HashSet<i64> {
    required.difference(columns).copied().collect()
}
/// 获取ColumnNamesFromIDs（对应同名 Go 逻辑）。
pub fn getColumnNamesFromIDs(columns: &[ColumnInfo], ids: &HashSet<i64>) -> Vec<String> {
    columns
        .iter()
        .filter(|c| ids.contains(&c.id))
        .map(|c| c.name.clone())
        .collect()
}
/// 获取ColumnListFromSet（对应同名 Go 逻辑）。
pub fn getColumnListFromSet(columns: &[ColumnInfo], ids: &HashSet<i64>) -> Vec<ColumnInfo> {
    columns
        .iter()
        .filter(|c| ids.contains(&c.id))
        .cloned()
        .collect()
}
/// 获取ColOffsetForAnalyze（对应同名 Go 逻辑）。
pub fn getColOffsetForAnalyze(columns: &[ColumnInfo], id: i64) -> Option<usize> {
    columns.iter().position(|c| c.id == id)
}
/// 过滤SkipColumnTypes（对应同名 Go 逻辑）。
pub fn filterSkipColumnTypes(
    columns: &[ColumnInfo],
    required: &HashSet<i64>,
) -> (Vec<ColumnInfo>, Vec<ColumnInfo>) {
    columns.iter().cloned().partition(|c| {
        required.contains(&c.id) || !matches!(c.field_type.code, TypeCode::Vector | TypeCode::Bytes)
    })
}
/// 合并AnalyzeOptions（对应同名 Go 逻辑）。
pub fn mergeAnalyzeOptions(
    mut statement: HashMap<AnalyzeOptionType, u64>,
    saved: &HashMap<AnalyzeOptionType, u64>,
) -> HashMap<AnalyzeOptionType, u64> {
    for (key, value) in saved {
        statement.entry(*key).or_insert(*value);
    }
    statement
}
/// 选取ColumnList（对应同名 Go 逻辑）。
pub fn pickColumnList(
    ast_choice: ColumnChoice,
    ast: Vec<ColumnInfo>,
    saved_choice: ColumnChoice,
    saved: Vec<ColumnInfo>,
) -> (ColumnChoice, Vec<ColumnInfo>) {
    if ast_choice == ColumnChoice::Default {
        (saved_choice, saved)
    } else {
        (ast_choice, ast)
    }
}
/// CMSketchSizeLimit：计划构建相关符号（对齐 Go 同名定义）。
pub const CMSketchSizeLimit: u64 = (6 << 20) / 5;
/// 获取AnalyzeOptionDefaultV2ForTest（对应同名 Go 逻辑）。
pub fn GetAnalyzeOptionDefaultV2ForTest() -> HashMap<AnalyzeOptionType, u64> {
    [
        (AnalyzeOptionType::Buckets, 256),
        (AnalyzeOptionType::TopN, 100),
        (AnalyzeOptionType::SampleRate, 1),
        (AnalyzeOptionType::CmsketchDepth, 5),
        (AnalyzeOptionType::CmsketchWidth, 2048),
    ]
    .into_iter()
    .collect()
}
/// 处理AnalyzeOptions（对应同名 Go 逻辑）。
pub fn handleAnalyzeOptions(
    options: &[(AnalyzeOptionType, u64)],
) -> Result<HashMap<AnalyzeOptionType, u64>> {
    let limits: HashMap<_, _> = [
        (AnalyzeOptionType::Buckets, 1024),
        (AnalyzeOptionType::TopN, 10_000),
        (AnalyzeOptionType::CmsketchDepth, 20),
        (AnalyzeOptionType::CmsketchWidth, CMSketchSizeLimit),
    ]
    .into_iter()
    .collect();
    let mut result = HashMap::new();
    for (key, value) in options {
        if *value == 0 || limits.get(key).is_some_and(|limit| value > limit) {
            return Err(BuilderError(format!(
                "invalid analyze option {key:?}={value}"
            )));
        }
        result.insert(*key, *value);
    }
    Ok(fillAnalyzeOptionsV2(result))
}
/// 填充AnalyzeOptionsV2（对应同名 Go 逻辑）。
pub fn fillAnalyzeOptionsV2(
    mut options: HashMap<AnalyzeOptionType, u64>,
) -> HashMap<AnalyzeOptionType, u64> {
    for (key, value) in GetAnalyzeOptionDefaultV2ForTest() {
        options.entry(key).or_insert(value);
    }
    options
}
/// 生成IndexTasks（对应同名 Go 逻辑）。
pub fn generateIndexTasks(
    index: &IndexMeta,
    ids: &[(i64, String)],
    version: i32,
) -> Vec<AnalyzeIndexTask> {
    ids.iter()
        .map(|(id, name)| AnalyzeIndexTask {
            index: index.clone(),
            physical_id: *id,
            partition_name: name.clone(),
            version,
        })
        .collect()
}

/// 构建ColumnWithName（对应同名 Go 逻辑）。
pub fn buildColumnWithName(name: &str, field_type: FieldType) -> (SchemaColumn, String) {
    (
        SchemaColumn {
            name: name.into(),
            field_type,
            flag: 0,
        },
        name.into(),
    )
}
/// columnsWithNames：计划构建相关符号（对齐 Go 同名定义）。
pub struct columnsWithNames {
    columns: Schema,
    names: Vec<String>,
}
impl columnsWithNames {
    /// 新建ColumnsWithNames（对应同名 Go 逻辑）。
    pub fn newColumnsWithNames(capacity: usize) -> Self {
        Self {
            columns: Vec::with_capacity(capacity),
            names: Vec::with_capacity(capacity),
        }
    }
    /// Append：计划构建相关符号（对齐 Go 同名定义）。
    pub fn Append(&mut self, column: SchemaColumn, name: String) {
        self.columns.push(column);
        self.names.push(name);
    }
    /// col2Schema：计划构建相关符号（对齐 Go 同名定义）。
    pub fn col2Schema(self) -> Schema {
        self.columns
    }
}
/// 新建ColumnsWithNames（对应同名 Go 逻辑）。
pub fn newColumnsWithNames(capacity: usize) -> columnsWithNames {
    columnsWithNames::newColumnsWithNames(capacity)
}
/// int_type：计划构建相关符号（对齐 Go 同名定义）。
fn int_type() -> FieldType {
    FieldType {
        code: TypeCode::Int,
        flen: 20,
        decimal: 0,
        unsigned: false,
    }
}
/// string_type：计划构建相关符号（对齐 Go 同名定义）。
fn string_type(size: i32) -> FieldType {
    FieldType {
        code: TypeCode::String,
        flen: size,
        decimal: 0,
        unsigned: false,
    }
}
/// schema：计划构建相关符号（对齐 Go 同名定义）。
fn schema(names: &[&str]) -> (Schema, Vec<String>) {
    let columns = names
        .iter()
        .map(|name| SchemaColumn {
            name: (*name).into(),
            field_type: string_type(256),
            flag: 0,
        })
        .collect();
    (columns, names.iter().map(|s| (*s).into()).collect())
}
/// 构建ShowNextRowID（对应同名 Go 逻辑）。
pub fn buildShowNextRowID() -> (Schema, Vec<String>) {
    schema(&[
        "DB_NAME",
        "TABLE_NAME",
        "COLUMN_NAME",
        "NEXT_GLOBAL_ROW_ID",
        "ID_TYPE",
    ])
}
/// 构建ShowDDLFields（对应同名 Go 逻辑）。
pub fn buildShowDDLFields() -> (Schema, Vec<String>) {
    schema(&[
        "SCHEMA_VER",
        "OWNER_ID",
        "OWNER_ADDRESS",
        "RUNNING_JOBS",
        "SELF_ID",
        "QUERY",
    ])
}
/// 构建RecoverIndexFields（对应同名 Go 逻辑）。
pub fn buildRecoverIndexFields() -> (Schema, Vec<String>) {
    schema(&["ADDED_COUNT", "SCAN_COUNT"])
}
/// 构建CleanupIndexFields（对应同名 Go 逻辑）。
pub fn buildCleanupIndexFields() -> (Schema, Vec<String>) {
    schema(&["REMOVED_COUNT"])
}
/// 构建ShowDDLJobsFields（对应同名 Go 逻辑）。
pub fn buildShowDDLJobsFields() -> (Schema, Vec<String>) {
    schema(&[
        "JOB_ID",
        "DB_NAME",
        "TABLE_NAME",
        "JOB_TYPE",
        "SCHEMA_STATE",
        "SCHEMA_ID",
        "TABLE_ID",
        "ROW_COUNT",
        "START_TIME",
        "STATE",
    ])
}
/// 构建TableDistributionSchema（对应同名 Go 逻辑）。
pub fn buildTableDistributionSchema() -> (Schema, Vec<String>) {
    schema(&["TABLE", "DISTRIBUTION"])
}
/// 构建TableRegionsSchema（对应同名 Go 逻辑）。
pub fn buildTableRegionsSchema() -> (Schema, Vec<String>) {
    schema(&[
        "REGION_ID",
        "START_KEY",
        "END_KEY",
        "LEADER_ID",
        "STORE_ID",
        "SCATTERING",
        "WRITTEN_BYTES",
        "READ_BYTES",
    ])
}
/// 构建SplitRegionsSchema（对应同名 Go 逻辑）。
pub fn buildSplitRegionsSchema() -> (Schema, Vec<String>) {
    schema(&["TOTAL_SPLIT_REGION", "SCATTER_FINISH_RATIO"])
}
/// 构建DistributeTableSchema（对应同名 Go 逻辑）。
pub fn buildDistributeTableSchema() -> (Schema, Vec<String>) {
    schema(&["RESULT"])
}
/// 构建ShowDDLJobQueriesFields（对应同名 Go 逻辑）。
pub fn buildShowDDLJobQueriesFields() -> (Schema, Vec<String>) {
    schema(&["JOB_ID", "QUERY"])
}
/// 构建ShowDDLJobQueriesWithRangeFields（对应同名 Go 逻辑）。
pub fn buildShowDDLJobQueriesWithRangeFields() -> (Schema, Vec<String>) {
    schema(&["JOB_ID", "QUERY", "START_TIME", "END_TIME"])
}
/// 构建ShowSlowSchema（对应同名 Go 逻辑）。
pub fn buildShowSlowSchema() -> (Schema, Vec<String>) {
    schema(&["SQL", "START", "DURATION", "DETAIL"])
}
/// 构建CommandOnDDLJobsFields（对应同名 Go 逻辑）。
pub fn buildCommandOnDDLJobsFields() -> (Schema, Vec<String>) {
    schema(&["JOB_ID", "RESULT"])
}
/// 构建CancelDDLJobsFields（对应同名 Go 逻辑）。
pub fn buildCancelDDLJobsFields() -> (Schema, Vec<String>) {
    buildCommandOnDDLJobsFields()
}
/// 构建PauseDDLJobsFields（对应同名 Go 逻辑）。
pub fn buildPauseDDLJobsFields() -> (Schema, Vec<String>) {
    buildCommandOnDDLJobsFields()
}
/// 构建ResumeDDLJobsFields（对应同名 Go 逻辑）。
pub fn buildResumeDDLJobsFields() -> (Schema, Vec<String>) {
    buildCommandOnDDLJobsFields()
}
/// 构建AdminShowBDRRoleFields（对应同名 Go 逻辑）。
pub fn buildAdminShowBDRRoleFields() -> (Schema, Vec<String>) {
    schema(&["ROLE"])
}
/// 构建ShowBackupMetaSchema（对应同名 Go 逻辑）。
pub fn buildShowBackupMetaSchema() -> (Schema, Vec<String>) {
    schema(&["DESTINATION", "BACKUP_TS", "SIZE"])
}
/// 构建ShowBackupQuerySchema（对应同名 Go 逻辑）。
pub fn buildShowBackupQuerySchema() -> (Schema, Vec<String>) {
    schema(&["QUERY", "STATUS", "PROGRESS"])
}
/// 构建BackupRestoreSchema（对应同名 Go 逻辑）。
pub fn buildBackupRestoreSchema(kind: &str) -> (Schema, Vec<String>) {
    schema(&[
        "JOB_ID",
        "DESTINATION",
        "STATE",
        "PROGRESS",
        if kind == "backup" {
            "BACKUP_TS"
        } else {
            "RESTORE_TS"
        },
    ])
}
/// 构建BRIESchema（对应同名 Go 逻辑）。
pub fn buildBRIESchema(kind: &str) -> (Schema, Vec<String>) {
    buildBackupRestoreSchema(kind)
}
/// 构建CalibrateResourceSchema（对应同名 Go 逻辑）。
pub fn buildCalibrateResourceSchema() -> (Schema, Vec<String>) {
    schema(&["QUOTA"])
}
/// 构建AddQueryWatchSchema（对应同名 Go 逻辑）。
pub fn buildAddQueryWatchSchema() -> (Schema, Vec<String>) {
    schema(&["WATCH_ID"])
}
/// 构建ShowTrafficJobsSchema（对应同名 Go 逻辑）。
pub fn buildShowTrafficJobsSchema() -> (Schema, Vec<String>) {
    schema(&["JOB_ID", "TYPE", "STATUS", "START_TIME", "END_TIME"])
}
/// 构建ShowProcedureSchema（对应同名 Go 逻辑）。
pub fn buildShowProcedureSchema() -> Schema {
    schema(&[
        "Db",
        "Name",
        "Type",
        "Definer",
        "Modified",
        "Created",
        "Security_type",
        "Comment",
        "character_set_client",
        "collation_connection",
        "Database Collation",
    ])
    .0
}
/// 构建ShowTriggerSchema（对应同名 Go 逻辑）。
pub fn buildShowTriggerSchema() -> Schema {
    schema(&[
        "Trigger",
        "Event",
        "Table",
        "Statement",
        "Timing",
        "Created",
        "sql_mode",
        "Definer",
        "character_set_client",
        "collation_connection",
        "Database Collation",
    ])
    .0
}
/// 构建ShowEventsSchema（对应同名 Go 逻辑）。
pub fn buildShowEventsSchema() -> Schema {
    schema(&[
        "Db",
        "Name",
        "Definer",
        "Time zone",
        "Type",
        "Execute at",
        "Interval value",
        "Interval field",
        "Starts",
        "Ends",
        "Status",
        "Originator",
        "character_set_client",
        "collation_connection",
        "Database Collation",
    ])
    .0
}
/// 构建ShowWarningsSchema（对应同名 Go 逻辑）。
pub fn buildShowWarningsSchema() -> Schema {
    schema(&["Level", "Code", "Message"]).0
}
/// 构建ShowSchema（对应同名 Go 逻辑）。
pub fn buildShowSchema(show: &ShowKind) -> Schema {
    match show {
        ShowKind::Warnings | ShowKind::Errors => buildShowWarningsSchema(),
        ShowKind::Slow => buildShowSlowSchema().0,
        ShowKind::Regions => buildTableRegionsSchema().0,
        ShowKind::Distribution => buildTableDistributionSchema().0,
        ShowKind::BackupMeta => buildShowBackupMetaSchema().0,
        ShowKind::BackupQuery => buildShowBackupQuerySchema().0,
        ShowKind::TrafficJobs => buildShowTrafficJobsSchema().0,
        ShowKind::Triggers => buildShowTriggerSchema(),
        ShowKind::Events => buildShowEventsSchema(),
        ShowKind::ProcedureStatus => buildShowProcedureSchema(),
        ShowKind::NextRowId => buildShowNextRowID().0,
        _ => schema(&["Name"]).0,
    }
}
/// convert2OutputSchemasAndNames：计划构建相关符号（对齐 Go 同名定义）。
pub fn convert2OutputSchemasAndNames(
    names: &[String],
    types: &[TypeCode],
    flags: &[u32],
) -> Result<(Schema, Vec<String>)> {
    if names.len() != types.len() || names.len() != flags.len() {
        return Err(BuilderError("schema arrays have different lengths".into()));
    }
    let columns = names
        .iter()
        .zip(types)
        .zip(flags)
        .map(|((name, code), flag)| SchemaColumn {
            name: name.clone(),
            field_type: FieldType {
                code: code.clone(),
                flen: 256,
                decimal: 0,
                unsigned: false,
            },
            flag: *flag,
        })
        .collect();
    Ok((columns, names.to_vec()))
}

/// splitWhere：计划构建相关符号（对齐 Go 同名定义）。
pub fn splitWhere(where_expr: &Expression) -> Vec<Expression> {
    where_expr
        .name
        .split(" and ")
        .map(|name| Expression {
            name: name.into(),
            ..where_expr.clone()
        })
        .collect()
}
/// 收集VisitInfoFromRevokeStmt（对应同名 Go 逻辑）。
pub fn collectVisitInfoFromRevokeStmt(
    mut visits: Vec<visitInfo>,
    db: &str,
    table: &str,
    privileges: &[Privilege],
) -> Vec<visitInfo> {
    visits.extend(privileges.iter().cloned().map(|privilege| visitInfo {
        privilege,
        db: db.into(),
        table: table.into(),
        column: String::new(),
        error: String::new(),
        alterWritable: false,
        dynamicPrivs: Vec::new(),
        dynamicWithGrant: false,
    }));
    visits
}
/// 追加VisitInfoIsRestrictedUser（对应同名 Go 逻辑）。
pub fn appendVisitInfoIsRestrictedUser(
    mut visits: Vec<visitInfo>,
    user: &str,
    privilege: &str,
) -> Vec<visitInfo> {
    visits.push(visitInfo {
        privilege: Privilege::Dynamic(privilege.into()),
        db: String::new(),
        table: user.into(),
        column: String::new(),
        error: "restricted user".into(),
        alterWritable: false,
        dynamicPrivs: vec!["RESTRICTED_USER_ADMIN".into()],
        dynamicWithGrant: false,
    });
    visits
}
/// 收集VisitInfoFromGrantStmt（对应同名 Go 逻辑）。
pub fn collectVisitInfoFromGrantStmt(
    visits: Vec<visitInfo>,
    db: &str,
    table: &str,
    privileges: &[Privilege],
) -> Vec<visitInfo> {
    collectVisitInfoFromRevokeStmt(visits, db, table, privileges)
}
/// 生成AuthErrForGrantStmt（对应同名 Go 逻辑）。
pub fn genAuthErrForGrantStmt(db: &str) -> BuilderError {
    BuilderError(format!(
        "access denied to grant privileges on database {db}"
    ))
}
/// 校验ImportIntoColAssignments（对应同名 Go 逻辑）。
pub fn checkImportIntoColAssignments(
    assignments: &[(String, Expression)],
) -> Result<HashMap<String, usize>> {
    let mut seen = HashMap::new();
    for (idx, (name, _)) in assignments.iter().enumerate() {
        if seen.insert(name.to_lowercase(), idx).is_some() {
            return Err(BuilderError(format!("duplicate assignment to {name}")));
        }
    }
    Ok(seen)
}
/// 填充DefaultDBForStatsObjects（对应同名 Go 逻辑）。
pub fn fillDefaultDBForStatsObjects(
    objects: &[(String, String)],
    default_db: &str,
) -> Result<Vec<(String, String)>> {
    objects
        .iter()
        .map(|(db, table)| {
            let db = if db.is_empty() { default_db } else { db };
            if db.is_empty() || table.is_empty() {
                Err(BuilderError("database and table are required".into()))
            } else {
                Ok((db.into(), table.clone()))
            }
        })
        .collect()
}
/// convertValue：计划构建相关符号（对齐 Go 同名定义）。
pub fn convertValue(value: &Value, column: &ColumnInfo) -> Result<Value> {
    match (&column.field_type.code, value) {
        (_, Value::Null | Value::Default) => Ok(value.clone()),
        (TypeCode::Int, Value::Int(_))
        | (TypeCode::UInt, Value::UInt(_))
        | (TypeCode::Float, Value::Float(_))
        | (TypeCode::String, Value::String(_))
        | (TypeCode::Bytes, Value::Bytes(_)) => Ok(value.clone()),
        (TypeCode::Int, Value::String(v)) => v
            .parse::<i64>()
            .map(Value::Int)
            .map_err(|_| BuilderError(format!("invalid integer for {}", column.name))),
        (TypeCode::UInt, Value::String(v)) => v
            .parse::<u64>()
            .map(Value::UInt)
            .map_err(|_| BuilderError(format!("invalid unsigned integer for {}", column.name))),
        (TypeCode::Float, Value::Int(v)) => Ok(Value::Float(*v as f64)),
        (TypeCode::String, other) => Ok(Value::String(format!("{other:?}"))),
        _ => Err(BuilderError(format!(
            "value cannot be converted for {}",
            column.name
        ))),
    }
}
/// 构建HandleColumnInfos（对应同名 Go 逻辑）。
pub fn buildHandleColumnInfos(table: &TableInfo) -> Vec<ColumnInfo> {
    BuildHandleColsForAnalyze(table)
}
/// convertValueListToData：计划构建相关符号（对齐 Go 同名定义）。
pub fn convertValueListToData(
    values: &[Value],
    columns: &[ColumnInfo],
    row: usize,
) -> Result<Vec<Value>> {
    if values.len() != columns.len() {
        return Err(BuilderError(format!("column count mismatch at row {row}")));
    }
    values
        .iter()
        .zip(columns)
        .map(|(v, c)| convertValue(v, c))
        .collect()
}
/// 校验ForUserVariables（对应同名 Go 逻辑）。
pub fn checkForUserVariables(value: &Value) -> Result<()> {
    if matches!(value, Value::UserVar(_)) {
        Err(BuilderError("user variables are not allowed here".into()))
    } else {
        Ok(())
    }
}
/// calcTSForPlanReplayer：计划构建相关符号（对齐 Go 同名定义）。
pub fn calcTSForPlanReplayer(value: Option<&Value>) -> u64 {
    match value {
        Some(Value::UInt(v)) => *v,
        Some(Value::Int(v)) if *v >= 0 => *v as u64,
        _ => 0,
    }
}
/// 构建ChecksumTableSchema（对应同名 Go 逻辑）。
pub fn buildChecksumTableSchema() -> Schema {
    schema(&[
        "Db_name",
        "Table_name",
        "Checksum_crc64_xor",
        "Total_kvs",
        "Total_bytes",
    ])
    .0
}
/// adjustOverlongViewColname：计划构建相关符号（对齐 Go 同名定义）。
pub fn adjustOverlongViewColname(names: &mut [String]) {
    for name in names {
        if name.len() > 64 {
            name.truncate(64);
        }
    }
}
/// 查找StmtAsViewSchema（对应同名 Go 逻辑）。
pub fn findStmtAsViewSchema(statement: &Statement) -> Option<&PlanNode> {
    match statement {
        Statement::Select { plan, .. } => Some(plan),
        Statement::Explain { stmt, .. }
        | Statement::Trace { stmt, .. }
        | Statement::SelectInto { source: stmt, .. } => findStmtAsViewSchema(stmt),
        _ => None,
    }
}
/// extractPatternLikeOrIlikeName：计划构建相关符号（对齐 Go 同名定义）。
pub fn extractPatternLikeOrIlikeName(pattern: &str) -> String {
    pattern
        .trim_matches(|c| c == '%' || c == '_')
        .replace("\\%", "%")
        .replace("\\_", "_")
}
/// 获取TablePath（对应同名 Go 逻辑）。
pub fn getTablePath(paths: &[AccessPath]) -> Option<&AccessPath> {
    paths.iter().find(|p| p.is_table_path())
}

#[derive(Clone, Debug)]
/// AlterDDLJobOpt：计划构建相关符号（对齐 Go 同名定义）。
pub enum AlterDDLJobOpt {
    Thread(i64),
    BatchSize(i64),
    MaxWriteSpeed(String),
}
/// 校验AlterDDLJobOptValue（对应同名 Go 逻辑）。
pub fn checkAlterDDLJobOptValue(option: &AlterDDLJobOpt) -> Result<()> {
    match option {
        AlterDDLJobOpt::Thread(v)
            if !(1..=vardef_dependency::MaxConfigurableConcurrency).contains(v) =>
        {
            Err(BuilderError(format!(
                "thread count {v} is out of range [1, {}]",
                vardef_dependency::MaxConfigurableConcurrency
            )))
        }
        AlterDDLJobOpt::BatchSize(v)
            if !(i64::from(vardef_dependency::MinDDLReorgBatchSize)
                ..=i64::from(vardef_dependency::MaxDDLReorgBatchSize))
                .contains(v) =>
        {
            Err(BuilderError(format!(
                "batch size {v} is out of range [{}, {}]",
                vardef_dependency::MinDDLReorgBatchSize,
                vardef_dependency::MaxDDLReorgBatchSize
            )))
        }
        AlterDDLJobOpt::MaxWriteSpeed(v) => {
            let speed = parse_size(v)?;
            if !(0..=(1_i64 << 50)).contains(&speed) {
                return Err(BuilderError(format!(
                    "max write speed {speed} is out of range [0, {}]",
                    1_i64 << 50
                )));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
/// 校验NextGenS3PathWithSem（对应同名 Go 逻辑）。
pub fn checkNextGenS3PathWithSem(path: &str) -> Result<()> {
    if !path.to_ascii_lowercase().starts_with("s3://") {
        return Err(BuilderError("only s3:// paths are allowed".into()));
    }

    let mut has_access_key = false;
    let mut has_secret_access_key = false;
    let mut has_role_arn = false;
    if let Some(query) = path.split_once('?').map(|(_, query)| query) {
        for parameter in query.split('&') {
            let (key, value) = parameter.split_once('=').unwrap_or((parameter, ""));
            let key = key.to_ascii_lowercase().replace('_', "-");
            match key.as_str() {
                "access-key" => has_access_key |= !value.is_empty(),
                "secret-access-key" => has_secret_access_key |= !value.is_empty(),
                "role-arn" => has_role_arn |= !value.is_empty(),
                // The standalone planner crate has no mutable global keyspace config. Its
                // default keyspace name is empty, so any explicit non-empty value differs.
                "external-id" if !value.is_empty() => {
                    return Err(BuilderError(
                        "explicit S3 external ID differs from the current keyspace".into(),
                    ));
                }
                _ => {}
            }
        }
    }
    if !has_role_arn && !(has_access_key && has_secret_access_key) {
        return Err(BuilderError(
            "S3 access key/secret access key or role ARN is required in SEM mode".into(),
        ));
    }
    Ok(())
}
/// 获取ThreadOrBatchSizeFromExpression（对应同名 Go 逻辑）。
pub fn GetThreadOrBatchSizeFromExpression(option: &AlterDDLJobOpt) -> Result<i64> {
    match option {
        AlterDDLJobOpt::Thread(v) | AlterDDLJobOpt::BatchSize(v) => Ok(*v),
        _ => Err(BuilderError("thread or batch-size option required".into())),
    }
}
/// 获取MaxWriteSpeedFromExpression（对应同名 Go 逻辑）。
pub fn GetMaxWriteSpeedFromExpression(option: &AlterDDLJobOpt) -> Result<i64> {
    match option {
        AlterDDLJobOpt::MaxWriteSpeed(value) => parse_size(value),
        _ => Err(BuilderError("max-write-speed option required".into())),
    }
}
/// parse_size：计划构建相关符号（对齐 Go 同名定义）。
fn parse_size(value: &str) -> Result<i64> {
    let value = value.trim().to_ascii_lowercase();
    let units = [
        ("pib", 1_i64 << 50),
        ("pb", 1_i64 << 50),
        ("tib", 1_i64 << 40),
        ("tb", 1_i64 << 40),
        ("gib", 1_i64 << 30),
        ("gb", 1_i64 << 30),
        ("mib", 1_i64 << 20),
        ("mb", 1_i64 << 20),
        ("kib", 1_i64 << 10),
        ("kb", 1_i64 << 10),
        ("b", 1),
    ];
    let (number, multiplier) = units
        .iter()
        .find_map(|(suffix, multiplier)| {
            value
                .strip_suffix(suffix)
                .map(|number| (number, *multiplier))
        })
        .unwrap_or((value.as_str(), 1));
    let number: f64 = number
        .trim()
        .parse()
        .map_err(|_| BuilderError("invalid size".into()))?;
    let bytes = number * multiplier as f64;
    if !bytes.is_finite() || bytes < 0.0 || bytes > i64::MAX as f64 {
        return Err(BuilderError("invalid or overflowing size".into()));
    }
    Ok(bytes as i64)
}
