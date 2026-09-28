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

// 规划器常见计划节点与辅助类型定义。
//
// 涵盖 Join/扫描/聚合等算子种类（`PlanKind`）、计划树节点（`PlanNode`）、
// DDL/管理/会话类计划结构，以及 EXPLAIN 结果渲染与点查（Point Get）判定。
// 执行计划（execution plan）描述 SQL 如何被物理算子树执行；本文件以迁移基线
// 形式保存这些结构，供 EXPLAIN、优化与执行侧共享。

use crate::{CIString, ExplainFlatPlanInRowFormat, FlattenPhysicalPlan, ToString};
use parser_ast_dependency::{FieldsClause, LinesClause};
use std::collections::{BTreeMap, BTreeSet};

/// Join 类型：半连接、外连接、内连接等。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JoinType {
    SemiJoin,
    AntiSemiJoin,
    LeftOuterSemiJoin,
    AntiLeftOuterSemiJoin,
    LeftOuterJoin,
    RightOuterJoin,
    InnerJoin,
}

/// 算子落点存储类型：Root（协处理器上层）、TiKV、TiFlash。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StoreType {
    #[default]
    Root,
    TiKV,
    TiFlash,
}

/// 计划算子种类枚举。
///
/// 变体携带扫描表名、连接等值条件、Limit 参数等轻量元数据，供 EXPLAIN 与树遍历使用。
#[derive(Clone, Debug, PartialEq)]
pub enum PlanKind {
    CheckTable,
    IndexScan {
        table: String,
        index: String,
        ranges: Vec<String>,
    },
    TableScan {
        table: String,
    },
    HashJoin {
        inner_child: usize,
        equal_conditions: Vec<(String, String)>,
    },
    MergeJoin {
        join_type: JoinType,
        keys: Vec<(String, String)>,
    },
    Apply,
    MaxOneRow,
    Limit {
        offset: u64,
        count: u64,
    },
    Lock,
    ShowDDL,
    Show {
        extractor: Option<String>,
    },
    ShowDDLJobs,
    Sort,
    Join {
        equal_conditions: Vec<(String, String)>,
    },
    UnionAll {
        partition: bool,
    },
    Sequence,
    DataSource {
        table: String,
        alias: Option<String>,
        partition_id: Option<i64>,
    },
    Selection {
        conditions: Vec<String>,
    },
    Projection,
    TopN {
        by_items: Vec<String>,
        offset: u64,
        count: u64,
    },
    Dual,
    HashAgg,
    StreamAgg,
    Aggregation {
        functions: Vec<String>,
    },
    TableReader,
    IndexReader,
    IndexLookUpReader,
    IndexMergeReader {
        partial_plans: Vec<PlanNode>,
        table_plan: Box<PlanNode>,
    },
    UnionScan {
        conditions: Vec<String>,
    },
    IndexJoin {
        keys: Vec<(String, String)>,
    },
    IndexMergeJoin {
        keys: Vec<(String, String)>,
    },
    IndexHashJoin {
        keys: Vec<(String, String)>,
    },
    Analyze {
        indexes: Vec<String>,
        columns: Vec<Vec<String>>,
    },
    Update,
    Delete,
    Insert,
    Window {
        functions: Vec<String>,
    },
    Shuffle {
        info: String,
    },
    ShuffleReceiver {
        info: String,
    },
    ExchangeReceiver {
        task_ids: Vec<i64>,
    },
    ExchangeSender {
        task_ids: Vec<i64>,
    },
    CTE {
        storage_id: i64,
    },
    CTEStorage,
    FKCheck,
    FKCascade,
    ScalarSubQuery,
    Generic(String),
}

/// 返回算子在 EXPLAIN 中的显示名称。
impl PlanKind {
    pub fn name(&self) -> &str {
        match self {
            Self::CheckTable => "CheckTable",
            Self::IndexScan { .. } => "IndexScan",
            Self::TableScan { .. } => "TableScan",
            Self::HashJoin { .. } => "HashJoin",
            Self::MergeJoin { .. } => "MergeJoin",
            Self::Apply => "Apply",
            Self::MaxOneRow => "MaxOneRow",
            Self::Limit { .. } => "Limit",
            Self::Lock => "Lock",
            Self::ShowDDL => "ShowDDL",
            Self::Show { .. } => "Show",
            Self::ShowDDLJobs => "ShowDDLJobs",
            Self::Sort => "Sort",
            Self::Join { .. } => "Join",
            Self::UnionAll { partition: true } => "PartitionUnionAll",
            Self::UnionAll { partition: false } => "UnionAll",
            Self::Sequence => "Sequence",
            Self::DataSource { .. } => "DataSource",
            Self::Selection { .. } => "Selection",
            Self::Projection => "Projection",
            Self::TopN { .. } => "TopN",
            Self::Dual => "Dual",
            Self::HashAgg => "HashAgg",
            Self::StreamAgg => "StreamAgg",
            Self::Aggregation { .. } => "Aggregation",
            Self::TableReader => "TableReader",
            Self::IndexReader => "IndexReader",
            Self::IndexLookUpReader => "IndexLookUpReader",
            Self::IndexMergeReader { .. } => "IndexMergeReader",
            Self::UnionScan { .. } => "UnionScan",
            Self::IndexJoin { .. } => "IndexJoin",
            Self::IndexMergeJoin { .. } => "IndexMergeJoin",
            Self::IndexHashJoin { .. } => "IndexHashJoin",
            Self::Analyze { .. } => "Analyze",
            Self::Update => "Update",
            Self::Delete => "Delete",
            Self::Insert => "Insert",
            Self::Window { .. } => "Window",
            Self::Shuffle { .. } => "Shuffle",
            Self::ShuffleReceiver { .. } => "ShuffleReceiver",
            Self::ExchangeReceiver { .. } => "ExchangeReceiver",
            Self::ExchangeSender { .. } => "ExchangeSender",
            Self::CTE { .. } => "CTE",
            Self::CTEStorage => "CTEStorage",
            Self::FKCheck => "FKCheck",
            Self::FKCascade => "FKCascade",
            Self::ScalarSubQuery => "ScalarSubQuery",
            Self::Generic(name) => name,
        }
    }
}

/// 计划树节点：算子种类、子节点、估算行数/代价与运行时统计。
#[derive(Clone, Debug, PartialEq)]
pub struct PlanNode {
    pub id: i32,
    pub kind: PlanKind,
    pub children: Vec<PlanNode>,
    pub store_type: StoreType,
    pub estimated_rows: f64,
    pub estimated_cost: f64,
    pub cost_formula: String,
    pub access_object: String,
    pub operator_info: String,
    pub fd: String,
    pub actual_rows: Option<u64>,
    pub execution_info: String,
    pub memory_bytes: i64,
    pub disk_bytes: i64,
    pub probe_count: f64,
    pub build_side: Option<usize>,
}

impl PlanNode {
    /// 构造默认字段填充的计划节点（存储类型 Root，探测计数 1）。
    pub fn New(id: i32, kind: PlanKind, children: Vec<PlanNode>) -> Self {
        Self {
            id,
            kind,
            children,
            store_type: StoreType::Root,
            estimated_rows: 0.0,
            estimated_cost: 0.0,
            cost_formula: String::new(),
            access_object: String::new(),
            operator_info: String::new(),
            fd: String::new(),
            actual_rows: None,
            execution_info: String::new(),
            memory_bytes: 0,
            disk_bytes: 0,
            probe_count: 1.0,
            build_side: None,
        }
    }

    /// 估算本节点（含字符串字段与子树）的内存占用字节数。
    pub fn MemoryUsage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64
            + self.children.iter().map(Self::MemoryUsage).sum::<i64>()
            + self.access_object.len() as i64
            + self.operator_info.len() as i64
    }

    /// 是否为物理算子（排除逻辑 DataSource / Join）。
    pub fn IsPhysical(&self) -> bool {
        !matches!(
            self.kind,
            PlanKind::DataSource { .. } | PlanKind::Join { .. }
        )
    }
}

/// 会话变量子集：事务、大小写、脏表等，供计划侧决策。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SessionVars {
    pub autocommit: bool,
    pub in_txn: bool,
    pub restricted_sql: bool,
    pub allow_auto_random_explicit_insert: bool,
    pub lower_case_table_names: i32,
    pub dirty_tables: BTreeSet<i64>,
    pub snapshot_tables: BTreeSet<i64>,
}

/// 规划上下文：会话变量与算子计数。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PlannerContext {
    pub vars: SessionVars,
    pub operator_num: Vec<u64>,
}

/// Schema 生产者：输出列名列表。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SchemaProducer {
    pub columns: Vec<String>,
}

/// 仅为 schema 字段的简单管理类计划生成结构体。
macro_rules! schema_plan {
    ($($name:ident),* $(,)?) => {$(
        #[derive(Clone, Debug, Default, Eq, PartialEq)]
        pub struct $name { pub schema: SchemaProducer }
    )*};
}

schema_plan!(
    ShowDDL,
    WorkloadRepoCreate,
    ReloadExprPushdownBlacklist,
    ReloadOptRuleBlacklist,
    AdminShowBDRRole
);

/// SHOW SLOW 计划。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ShowSlow {
    pub schema: SchemaProducer,
    pub show_slow: String,
}
/// 按 JobID 查询 DDL Job 详情。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ShowDDLJobQueries {
    pub schema: SchemaProducer,
    pub JobIDs: Vec<i64>,
}
/// 带 Limit/Offset 的 DDL Job 查询。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ShowDDLJobQueriesWithRange {
    pub schema: SchemaProducer,
    pub Limit: u64,
    pub Offset: u64,
}
/// SHOW NEXT_ROW_ID 计划。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ShowNextRowID {
    pub schema: SchemaProducer,
    pub TableName: String,
}
/// CHECK TABLE 计划。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CheckTable {
    pub schema: SchemaProducer,
    pub Tables: Vec<String>,
    pub CheckIndex: bool,
}

/// 表+索引名的管理计划（RecoverIndex / CleanupIndex）。
macro_rules! table_index_plan {
    ($($name:ident),* $(,)?) => {$(
        #[derive(Clone, Debug, Default, Eq, PartialEq)]
        pub struct $name { pub schema: SchemaProducer, pub Table: String, pub IndexName: String }
    )*};
}
table_index_plan!(RecoverIndex, CleanupIndex);

/// 按 Handle 范围检查索引。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CheckIndexRange {
    pub schema: SchemaProducer,
    pub Table: String,
    pub IndexName: String,
    pub HandleRanges: Vec<(i64, i64)>,
}
/// CHECKSUM TABLE 计划。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChecksumTable {
    pub schema: SchemaProducer,
    pub Tables: Vec<String>,
}

/// 按 JobID 列表取消/暂停/恢复 DDL。
macro_rules! ddl_jobs_plan {
    ($($name:ident),* $(,)?) => {$(
        #[derive(Clone, Debug, Default, Eq, PartialEq)]
        pub struct $name { pub schema: SchemaProducer, pub JobIDs: Vec<i64> }
    )*};
}
ddl_jobs_plan!(CancelDDLJobs, PauseDDLJobs, ResumeDDLJobs);

/// Alter DDL Job 允许的参数名：并发线程数。
pub const AlterDDLJobThread: &str = "thread";
/// Alter DDL Job 允许的参数名：批大小。
pub const AlterDDLJobBatchSize: &str = "batch_size";
/// Alter DDL Job 允许的参数名：最大写入速度。
pub const AlterDDLJobMaxWriteSpeed: &str = "max_write_speed";
/// 返回允许修改的 Alter DDL Job 参数名集合。
pub fn allowedAlterDDLJobParams() -> BTreeSet<&'static str> {
    [
        AlterDDLJobThread,
        AlterDDLJobBatchSize,
        AlterDDLJobMaxWriteSpeed,
    ]
    .into_iter()
    .collect()
}

/// 单条 Alter DDL Job 选项（名=值）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlterDDLJobOpt {
    pub Name: String,
    pub Value: String,
}
/// ALTER DDL JOB 计划。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AlterDDLJob {
    pub schema: SchemaProducer,
    pub JobID: i64,
    pub Options: Vec<AlterDDLJobOpt>,
}

/// ADMIN PLUGINS 动作：启用或禁用。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdminPluginsAction {
    Enable = 1,
    Disable = 2,
}
/// ADMIN PLUGINS 计划。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdminPlugins {
    pub schema: SchemaProducer,
    pub Action: AdminPluginsAction,
    pub Plugins: Vec<String>,
}
/// PREPARE 语句计划。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Prepare {
    pub schema: SchemaProducer,
    pub Name: String,
    pub SQLText: String,
}
/// EXECUTE 语句计划（可携带已解析的物理计划）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Execute {
    pub schema: SchemaProducer,
    pub Name: String,
    pub UsingVars: Vec<String>,
    pub BinaryArgs: Vec<String>,
    pub PrepStmt: Option<String>,
    pub Plan: Option<PlanNode>,
}
/// DEALLOCATE 语句计划。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Deallocate {
    pub schema: SchemaProducer,
    pub Name: String,
}
/// SET 变量赋值计划。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Set {
    pub schema: SchemaProducer,
    pub VarAssigns: Vec<(String, String)>,
}
/// SET CONFIG 计划。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SetConfig {
    pub schema: SchemaProducer,
    pub Type: String,
    pub Instance: String,
    pub Name: String,
    pub Value: String,
}
/// 索引推荐（Recommend Index）计划。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecommendIndexPlan {
    pub schema: SchemaProducer,
    pub Action: String,
    pub SQL: String,
    pub AdviseID: i64,
    pub Options: Vec<(String, String)>,
}

/// SQL Binding 操作类型（全局/会话创建删除与 Flush）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SQLBindOpType {
    CreateGlobal,
    DropGlobal,
    SetGlobalStatus,
    CreateSession,
    DropSession,
    Flush,
}
/// 单条 Binding 详情。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SQLBindOpDetail {
    pub OriginalSQL: String,
    pub BindSQL: String,
    pub Db: String,
    pub Charset: String,
    pub Collation: String,
    pub Source: String,
}
/// SQL Binding 管理计划。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SQLBindPlan {
    pub schema: SchemaProducer,
    pub BindOp: SQLBindOpType,
    pub Details: Vec<SQLBindOpDetail>,
}

/// 简单语句计划（如 USE / 其它非查询语句）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Simple {
    pub schema: SchemaProducer,
    pub Statement: String,
    pub ResolveCtx: BTreeMap<String, String>,
}
/// 物理计划包装器。
#[derive(Clone, Debug, PartialEq)]
pub struct PhysicalPlanWrapper {
    pub Inner: PlanNode,
}
impl PhysicalPlanWrapper {
    /// 包装器本身加内层计划的内存占用。
    pub fn MemoryUsage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64 + self.Inner.MemoryUsage()
    }
}

/// ANALYZE 目标表元信息。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AnalyzeInfo {
    pub DBName: String,
    pub TableName: String,
    pub PartitionName: String,
    pub TableID: i64,
    pub StatsVersion: i32,
    pub V2Options: Option<V2AnalyzeOptions>,
}
/// ANALYZE V2 选项。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct V2AnalyzeOptions {
    pub FilledOpts: BTreeMap<String, u64>,
    pub ColumnChoice: String,
    pub ColumnList: Vec<String>,
    pub RawOpts: BTreeMap<String, String>,
}
/// 列统计收集任务。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AnalyzeColumnsTask {
    pub HandleCols: Option<String>,
    pub ColsInfo: Vec<CIString>,
    pub AnalyzeInfo: AnalyzeInfo,
}
/// 索引统计收集任务。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AnalyzeIndexTask {
    pub IndexInfo: CIString,
    pub TblInfo: CIString,
    pub AnalyzeInfo: AnalyzeInfo,
}
/// ANALYZE 计划：索引任务、列任务与选项。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Analyze {
    pub IdxTasks: Vec<AnalyzeIndexTask>,
    pub ColTasks: Vec<AnalyzeColumnsTask>,
    pub Options: BTreeMap<String, u64>,
}

/// LOAD DATA 计划。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LoadData {
    pub Plan: Option<crate::InitializedPlan>,
    pub Path: String,
    pub Table: String,
    pub Columns: Vec<String>,
    pub IgnoreLines: u64,
    pub Options: Vec<LoadDataOpt>,
}
/// LOAD DATA / IMPORT INTO 的名值选项。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LoadDataOpt {
    pub Name: String,
    pub Value: String,
}
/// IMPORT INTO 计划。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ImportInto {
    pub Plan: Option<crate::InitializedPlan>,
    pub Path: String,
    pub Table: String,
    pub Options: Vec<LoadDataOpt>,
}

/// 仅含路径的计划（LoadStats / PlanReplayer）。
macro_rules! path_plan {
    ($($name:ident),* $(,)?) => {$(
        #[derive(Clone, Debug, Default, Eq, PartialEq)]
        pub struct $name { pub schema: SchemaProducer, pub Path: String }
    )*};
}
path_plan!(LoadStats, PlanReplayer);
/// LOCK STATS 计划。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LockStats {
    pub schema: SchemaProducer,
    pub Tables: Vec<String>,
}
/// UNLOCK STATS 与 LOCK STATS 结构相同。
pub type UnlockStats = LockStats;

/// TRAFFIC 捕获/回放相关计划。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Traffic {
    pub schema: SchemaProducer,
    pub OpType: String,
    pub Options: Vec<(String, String)>,
    pub Dir: String,
}

/// DISTRIBUTE TABLE 计划（分区与放置规则）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DistributeTable {
    pub schema: SchemaProducer,
    pub TableInfo: String,
    pub PartitionNames: Vec<CIString>,
    pub Engine: String,
    pub Rule: String,
    pub Timeout: String,
}

/// SPLIT REGION 计划。
///
/// Region 是键空间分片；本计划按范围或取值列表触发分裂。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SplitRegion {
    pub schema: SchemaProducer,
    pub TableInfo: String,
    pub PartitionNames: Vec<CIString>,
    pub IndexInfo: Option<String>,
    pub Lower: Vec<String>,
    pub Upper: Vec<String>,
    pub Num: i32,
    pub ValueLists: Vec<Vec<String>>,
}

/// 查询 SPLIT REGION 状态。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SplitRegionStatus {
    pub schema: SchemaProducer,
    pub Table: String,
    pub IndexInfo: Option<String>,
}

/// COMPACT TABLE 计划。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompactTable {
    pub schema: SchemaProducer,
    pub ReplicaKind: String,
    pub TableInfo: String,
    pub PartitionNames: Vec<CIString>,
}

/// 通用 DDL 语句计划。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DDL {
    pub schema: SchemaProducer,
    pub Statement: String,
}

/// SELECT ... INTO 导出计划。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SelectInto {
    pub schema: SchemaProducer,
    pub TargetPlan: Option<PlanNode>,
    pub IntoOpt: String,
    pub LineFieldsInfo: LineFieldsInfo,
}

/// LOAD DATA / SELECT INTO 的字段与行分隔配置。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LineFieldsInfo {
    pub FieldsTerminatedBy: String,
    pub FieldsEnclosedBy: String,
    pub FieldsEscapedBy: String,
    pub FieldsOptEnclosed: bool,
    pub LinesStartingBy: String,
    pub LinesTerminatedBy: String,
}
/// 从 AST 的 Fields/Lines 子句构造 `LineFieldsInfo`，缺省为制表符字段与换行行终止。
pub fn NewLineFieldsInfo(
    fields: Option<&FieldsClause>,
    lines: Option<&LinesClause>,
) -> LineFieldsInfo {
    let mut info = LineFieldsInfo {
        FieldsTerminatedBy: "\t".into(),
        FieldsEnclosedBy: String::new(),
        FieldsEscapedBy: "\\".into(),
        FieldsOptEnclosed: false,
        LinesStartingBy: String::new(),
        LinesTerminatedBy: "\n".into(),
    };
    if let Some(fields) = fields {
        if let Some(terminated) = &fields.Terminated {
            info.FieldsTerminatedBy = terminated.clone();
        }
        if let Some(enclosed) = &fields.Enclosed {
            info.FieldsEnclosedBy = enclosed.clone();
        }
        if let Some(escaped) = &fields.Escaped {
            info.FieldsEscapedBy = escaped.clone();
        }
        info.FieldsOptEnclosed = fields.OptEnclosed;
    }
    if let Some(lines) = lines {
        if let Some(starting) = &lines.Starting {
            info.LinesStartingBy = starting.clone();
        }
        if let Some(terminated) = &lines.Terminated {
            info.LinesTerminatedBy = terminated.clone();
        }
    }
    info
}

/// 供 JSON 编码的 EXPLAIN 行信息（含递归子节点）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExplainInfoForEncode {
    pub ID: String,
    pub EstRows: String,
    pub ActRows: String,
    pub TaskType: String,
    pub AccessObject: String,
    /// Go 字段名为 `executeInfo`；保留 `ExecutionInfo` 作为旧 Rust 调用方兼容输入。
    pub ExecuteInfo: String,
    pub ExecutionInfo: String,
    pub OperatorInfo: String,
    pub EstCost: String,
    pub CostFormula: String,
    pub MemoryInfo: String,
    pub DiskInfo: String,
    pub TotalMemoryConsumed: String,
    pub SubOperators: Vec<ExplainInfoForEncode>,
    /// `SubOperators` 的旧 Rust 别名，仅用于兼容已有调用方。
    pub Children: Vec<ExplainInfoForEncode>,
}

/// 对 JSON 字符串值做必要转义。
fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\u{08}' => escaped.push_str("\\b"),
            '\u{0c}' => escaped.push_str("\\f"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\u{2028}' => escaped.push_str("\\u2028"),
            '\u{2029}' => escaped.push_str("\\u2029"),
            c if c <= '\u{1f}' => escaped.push_str(&format!("\\u{:04x}", c as u32)),
            c => escaped.push(c),
        }
    }
    escaped
}
/// 递归将 EXPLAIN 行编码为 JSON 对象字符串。
fn encode_explain(row: &ExplainInfoForEncode, depth: usize) -> String {
    let indent = "    ".repeat(depth);
    let field_indent = "    ".repeat(depth + 1);
    let mut fields = Vec::new();
    let mut push_string = |name: &str, value: &str, omit_empty: bool| {
        if !omit_empty || !value.is_empty() {
            fields.push(format!(
                "{field_indent}\"{name}\": \"{}\"",
                json_escape(value)
            ));
        }
    };
    push_string("id", &row.ID, false);
    push_string("estRows", &row.EstRows, false);
    push_string("actRows", &row.ActRows, true);
    push_string("taskType", &row.TaskType, false);
    push_string("accessObject", &row.AccessObject, true);
    let execute_info = if row.ExecuteInfo.is_empty() {
        &row.ExecutionInfo
    } else {
        &row.ExecuteInfo
    };
    push_string("executeInfo", execute_info, true);
    push_string("operatorInfo", &row.OperatorInfo, true);
    push_string("estCost", &row.EstCost, true);
    push_string("costFormula", &row.CostFormula, true);
    push_string("memoryInfo", &row.MemoryInfo, true);
    push_string("diskInfo", &row.DiskInfo, true);
    push_string("totalMemoryConsumed", &row.TotalMemoryConsumed, true);

    let sub_operators = if row.SubOperators.is_empty() {
        &row.Children
    } else {
        &row.SubOperators
    };
    if !sub_operators.is_empty() {
        let children = sub_operators
            .iter()
            .map(|child| encode_explain(child, depth + 2))
            .collect::<Vec<_>>()
            .join(",\n");
        fields.push(format!(
            "{field_indent}\"subOperators\": [\n{children}\n{field_indent}]"
        ));
    }
    format!("{indent}{{\n{}\n{indent}}}", fields.join(",\n"))
}
/// 将多行 EXPLAIN 信息编码为 JSON 数组字符串。
pub fn JSONToString(rows: &[ExplainInfoForEncode]) -> String {
    if rows.is_empty() {
        return "[]\n".to_owned();
    }
    let encoded = rows
        .iter()
        .map(|row| encode_explain(row, 1))
        .collect::<Vec<_>>()
        .join(",\n");
    format!("[\n{encoded}\n]\n")
}

/// EXPLAIN / EXPLAIN ANALYZE 计划容器。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Explain {
    pub TargetPlan: Option<PlanNode>,
    pub Format: String,
    pub Analyze: bool,
    pub Rows: Vec<Vec<String>>,
    pub BriefBinaryPlan: bool,
}

/// 物理计划与是否为 Index Nested Loop 子侧的配对。
#[derive(Clone, Debug, PartialEq)]
pub struct PlanPair {
    pub physicalPlan: PlanNode,
    pub isChildOfINL: bool,
}
impl Explain {
    /// 扁平化目标计划并按 Format 渲染为行结果。
    pub fn RenderResult(&mut self) -> Result<(), String> {
        let target = self
            .TargetPlan
            .as_ref()
            .ok_or_else(|| "explain target plan is missing".to_owned())?;
        let flat = FlattenPhysicalPlan(Some(target), false)
            .ok_or_else(|| "cannot flatten an empty plan".to_owned())?;
        self.Rows = ExplainFlatPlanInRowFormat(&flat, &self.Format, self.Analyze);
        Ok(())
    }
}
/// 将计划 `ToString` 结果转为十六进制简述二进制串。
pub fn GetBriefBinaryPlan(plan: Option<&PlanNode>) -> String {
    plan.map(ToString)
        .unwrap_or_default()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
/// 渲染并返回 EXPLAIN ANALYZE 行。
pub fn GetExplainAnalyzeRowsForPlan(plan: &mut Explain) -> Vec<Vec<String>> {
    let _ = plan.RenderResult();
    plan.Rows.clone()
}

/// 是否处于自动提交且不在显式事务中。
pub fn IsAutoCommitTxn(vars: &SessionVars) -> bool {
    vars.autocommit && !vars.in_txn
}
/// 判断是否为自动提交下的主键/唯一键点查（Point Get）或其可穿透的 Projection/Lock 包装。
///
/// 点查指通过主键或唯一索引一次定位单行；此类计划在 autocommit 下可走快速路径。
pub fn IsPointGetWithPKOrUniqueKeyByAutoCommit(vars: &SessionVars, plan: &PlanNode) -> bool {
    if !IsAutoCommitTxn(vars) {
        return false;
    }
    match &plan.kind {
        PlanKind::IndexScan { ranges, .. } => ranges.len() == 1,
        PlanKind::Generic(name) if name == "PointGet" || name == "BatchPointGet" => true,
        PlanKind::Projection | PlanKind::Lock => plan
            .children
            .first()
            .is_some_and(|child| IsPointGetWithPKOrUniqueKeyByAutoCommit(vars, child)),
        _ => false,
    }
}
