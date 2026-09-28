// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// `SHOW` 语句统一执行器：按语句类型拉取元数据/统计/运维信息并编码为行。
//
// 负责分发、过滤、排序与 Chunk 写出；环境相关能力经 `ShowRuntimeContext`
// 边界注入。Region 指 TiKV 键空间分片；Placement 描述副本放置策略。

#![allow(dead_code, non_camel_case_types, non_snake_case)]

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use astersql_errors as errors;
use astersql_session_sessmgr::{Manager as SessionManager, ProcessListValue};
use astersql_util_chunk as chunk;

/// SHOW 路径统一结果类型。
pub type ShowResult<T = ()> = Result<T, errors::SharedError>;
/// 测试注入：覆盖集群配置 SHOW 的取数函数。
pub type TestShowClusterConfigFunc = Arc<dyn Fn() -> ShowResult<Vec<Vec<ShowValue>>> + Send + Sync>;

/// 结果单元格：空值与常见标量类型。
#[derive(Clone, Debug, PartialEq)]
pub enum ShowValue {
    Null,
    Int64(i64),
    Uint64(u64),
    Float64(f64),
    String(String),
    Bytes(Vec<u8>),
}

impl From<&str> for ShowValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<String> for ShowValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

/// `SHOW` 语句种类（解析/计划侧传入执行器）。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ShowStmtType {
    Charset,
    Collation,
    Columns,
    Config,
    CreateTable,
    CreateSequence,
    CreateUser,
    CreateView,
    CreateDatabase,
    CreatePlacementPolicy,
    CreateResourceGroup,
    Databases,
    Engines,
    Grants,
    Index,
    ProcedureStatus,
    Status,
    Tables,
    OpenTables,
    TableStatus,
    Triggers,
    Variables,
    Warnings,
    Errors,
    ProcessList,
    Events,
    StatsExtended,
    StatsMeta,
    StatsHistograms,
    StatsBuckets,
    StatsTopN,
    StatsHealthy,
    StatsLocked,
    HistogramsInFlight,
    ColumnStatsUsage,
    Plugins,
    Profiles,
    MasterStatus,
    BinlogStatus,
    Privileges,
    Bindings,
    BindingCacheStatus,
    AnalyzeStatus,
    Regions,
    Distributions,
    Builtins,
    Backups,
    Restores,
    PlacementLabels,
    Placement,
    PlacementForDatabase,
    PlacementForTable,
    PlacementForPartition,
    SessionStates,
    ImportJobs,
    ImportGroups,
    DistributionJobs,
    Affinity,
}

/// WHERE/LIKE 等过滤谓词（精确字段或 LIKE 模式）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ShowPredicate {
    pub field: Option<String>,
    pub like_pattern: Option<String>,
}

/// 带可选分区名列表的库表引用。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableName {
    pub schema: String,
    pub name: String,
    pub partition_names: Vec<String>,
}

/// 用户身份：用户名与主机名。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserIdentity {
    pub username: String,
    pub hostname: String,
}

/// 一次 SHOW 调用的完整请求参数快照。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShowRequest {
    pub statement: ShowStmtType,
    pub database: String,
    pub table: Option<TableName>,
    pub partition: String,
    pub column: Option<String>,
    pub index_name: String,
    pub resource_group_name: String,
    pub flag: i32,
    pub roles: Vec<String>,
    pub user: Option<UserIdentity>,
    pub extractor: ShowPredicate,
    pub count_warnings_or_errors: bool,
    pub full: bool,
    pub if_not_exists: bool,
    pub global_scope: bool,
    pub extended: bool,
    pub import_job_id: Option<i64>,
    pub distribution_job_id: Option<i64>,
    pub import_group_key: String,
}

/// 运行时取数操作码，与 `ShowStmtType` 对应但面向 `FetchRows`。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ShowOperation {
    Bindings,
    BindingCacheStatus,
    Engines,
    Databases,
    ProcessList,
    OpenTables,
    Tables,
    TableStatus,
    Columns,
    Index,
    Charset,
    MasterStatus,
    Variables,
    Status,
    CreateSequence,
    ClusterConfigs,
    CreateTable,
    CreateView,
    CreateDatabase,
    CreatePlacementPolicy,
    CreateResourceGroup,
    Collation,
    CreateUser,
    Grants,
    Privileges,
    Triggers,
    ProcedureStatus,
    Plugins,
    Warnings,
    Errors,
    Distributions,
    TableRegions,
    Builtins,
    SessionStates,
    DistributionJobs,
    ImportGroups,
    ImportJobs,
    StatsMeta,
    StatsHistograms,
    StatsBuckets,
    StatsTopN,
    StatsHealthy,
    StatsLocked,
    HistogramsInFlight,
    ColumnStatsUsage,
    AnalyzeStatus,
    Backups,
    Restores,
    PlacementLabels,
    Placement,
    PlacementForDatabase,
    PlacementForTable,
    PlacementForPartition,
    Affinity,
}

/// 表对象种类：基表、视图、序列或系统视图。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableKind {
    BaseTable,
    View,
    Sequence,
    SystemView,
}

/// 轻量表元信息，用于 TABLES 列表与 CREATE 语句构造。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableInfo {
    pub id: i64,
    pub name: String,
    pub kind: TableKind,
    pub temporary: bool,
}

/// SHOW TABLES 行：表名与可选 TableType。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct showInfo {
    pub Name: String,
    pub TableType: String,
}

/// Region 副本 Peer 标识。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RegionPeer {
    pub id: u64,
}

/// Region 元数据：起止键、leader、Store 与读写统计等。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct regionMeta {
    pub id: u64,
    pub physical_id: i64,
    pub start: String,
    pub end: String,
    pub leader_id: u64,
    pub store_id: u64,
    pub peers: Vec<RegionPeer>,
    pub scattering: bool,
    pub written_bytes: u64,
    pub read_bytes: u64,
    pub approximate_size: i64,
    pub approximate_keys: i64,
}

/// SHOW TABLE REGIONS 一行：Region 元数据加调度约束/状态。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct showTableRegionRowItem {
    pub regionMeta: regionMeta,
    pub schedulingConstraints: String,
    pub schedulingState: String,
}

/// 按 Store 汇总的 Region 分布与读写指标。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RegionDistribution {
    pub store_id: u64,
    pub engine_type: String,
    pub region_leader_count: i64,
    pub region_peer_count: i64,
    pub region_write_bytes: u64,
    pub region_write_keys: u64,
    pub region_write_query: u64,
    pub region_leader_read_bytes: u64,
    pub region_leader_read_keys: u64,
    pub region_leader_read_query: u64,
    pub region_peer_read_bytes: u64,
    pub region_peer_read_keys: u64,
    pub region_peer_read_query: u64,
}

/// 运行中 IMPORT 作业的运行时进度信息。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ImportRuntimeInfo {
    pub import_rows: u64,
    pub update_time: Option<SystemTime>,
    pub step: String,
    pub processed_size: String,
    pub total_size: String,
    pub percent: String,
    pub speed: String,
    pub eta: String,
    pub awaiting_resolution_error: Option<String>,
}

/// IMPORT INTO 作业的持久化/展示信息。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ImportJobInfo {
    pub id: i64,
    pub group_key: String,
    pub file_location: String,
    pub table_schema: String,
    pub table_name: String,
    pub table_id: i64,
    pub step: String,
    pub status: String,
    pub source_file_size: Option<u64>,
    pub imported_rows: u64,
    pub conflict_row_count: u64,
    pub too_many_conflicts: bool,
    pub error_message: String,
    pub create_time: Option<SystemTime>,
    pub start_time: Option<SystemTime>,
    pub end_time: Option<SystemTime>,
    pub update_time: Option<SystemTime>,
    pub created_by: String,
}

impl ImportJobInfo {
    /// 作业是否已成功结束。
    fn IsSuccess(&self) -> bool {
        self.status == "finished"
    }
}

/// 数据分布（Distribution）作业展示模型。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DistributionJob {
    pub job_id: u64,
    pub alias: String,
    pub engine: String,
    pub rule: String,
    pub status: String,
    pub timeout: Duration,
    pub create: Option<SystemTime>,
    pub start: Option<SystemTime>,
    pub finish: Option<SystemTime>,
}

/// IMPORT 作业按 group_key 聚合后的计数与时间。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct groupInfo {
    pub groupKey: String,
    pub jobCount: i64,
    pub pending: i64,
    pub running: i64,
    pub completed: i64,
    pub failed: i64,
    pub canceled: i64,
    pub createTime: Option<SystemTime>,
    pub updateTime: Option<SystemTime>,
}

/// 构造 SHOW CREATE TABLE/VIEW/SEQUENCE 的输入。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateTableInput {
    pub database: Option<String>,
    pub table: TableInfo,
    pub if_not_exists: bool,
}

/// 构造 SHOW CREATE DATABASE 的输入。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateDatabaseInput {
    pub name: String,
    pub charset: String,
    pub collation: String,
    pub placement_policy: Option<String>,
    pub if_not_exists: bool,
}

/// Placement Policy（放置策略）名称与设置片段。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlacementPolicyInfo {
    pub name: String,
    pub settings: String,
}

/// Resource Group（资源组）名称与设置片段。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceGroupInfo {
    pub name: String,
    pub settings: String,
}

/// 将表/序列/视图格式化为 CREATE 语句文本。
pub trait ShowCreateFormatter: Send + Sync {
    fn FormatCreateTable(&self, input: &CreateTableInput) -> ShowResult<String>;
    fn FormatCreateSequence(&self, input: &CreateTableInput) -> ShowResult<String>;
    fn FormatCreateView(&self, input: &CreateTableInput) -> ShowResult<String>;
}

/// All environment-dependent SHOW work is mandatory at this boundary. The
/// executor owns dispatch, filtering, ordering, row encoding and aggregation.
/// 环境相关 SHOW 能力的强制边界；执行器只负责分发、过滤、排序、编码与聚合。
pub trait ShowRuntimeContext: Send + Sync {
    fn MaxChunkSize(&self) -> usize;
    fn SelectLimit(&self) -> u64;
    fn SetSelectLimit(&self, value: u64);
    fn FetchRows(
        &self,
        operation: ShowOperation,
        request: &ShowRequest,
    ) -> ShowResult<Vec<Vec<ShowValue>>>;
    fn AllSchemaNames(&self) -> ShowResult<Vec<String>>;
    fn DatabaseVisible(&self, database: &str) -> ShowResult<bool>;
    fn SchemaExists(&self, database: &str) -> ShowResult<bool>;
    fn Tables(&self, database: &str, full: bool) -> ShowResult<Vec<TableInfo>>;
    fn TableByName(&self, database: &str, table: &str) -> ShowResult<Option<TableInfo>>;
    fn TableVisible(&self, database: &str, table: &str) -> ShowResult<bool>;
    fn TableRegions(&self, table: &TableName, physical_ids: &[i64]) -> ShowResult<Vec<regionMeta>>;
    fn IndexRegions(
        &self,
        table: &TableName,
        index_name: &str,
        physical_ids: &[i64],
    ) -> ShowResult<Vec<regionMeta>>;
    fn SchedulingInfo(
        &self,
        regions: &[regionMeta],
        table: &TableName,
    ) -> ShowResult<Vec<showTableRegionRowItem>>;
    fn ImportJobs(&self, request: &ShowRequest) -> ShowResult<Vec<ImportJobInfo>>;
    fn ImportRuntime(&self, job_id: i64) -> ShowResult<Option<ImportRuntimeInfo>>;
    fn DistributionJobs(&self, request: &ShowRequest) -> ShowResult<Vec<DistributionJob>>;
    fn RunWithSystemSession(
        &self,
        action: &mut dyn FnMut(&dyn ShowRuntimeContext) -> ShowResult,
    ) -> ShowResult;
    fn FillViewColumnType(&self, database: &str, table: &TableInfo) -> ShowResult;
    fn FormatSplitExpression(&self, expression: &str) -> ShowResult<String>;

    /// 当前会话持有的 Server SessionManager；nil 与 Go 一样产生空结果。
    fn GetSessionManager(&self) -> Option<Arc<dyn SessionManager>> {
        None
    }

    /// 登录用户名，对齐 `SessionVars.User.Username`。
    fn LoginUserName(&self) -> Option<String> {
        None
    }

    /// 当前 active roles 是否拥有 PROCESS 权限。
    fn HasProcessPrivilege(&self) -> bool {
        false
    }
}

/// 对齐 Go `ShowExec.fetchShowProcessList`：从 SessionManager 取快照，
/// 无 PROCESS 权限时仅保留当前登录用户，再调用 `ToRowForShow` 编码。
pub fn FetchShowProcessListRows(
    manager: Option<&dyn SessionManager>,
    login_user: Option<&str>,
    has_process_privilege: bool,
    full: bool,
) -> Vec<Vec<ShowValue>> {
    let Some(manager) = manager else {
        return Vec::new();
    };
    let login_user = login_user.unwrap_or_default();
    manager
        .ShowProcessList()
        .into_values()
        .filter(|process| has_process_privilege || process.User == login_user)
        .map(|process| {
            process
                .ToRowForShow(full)
                .into_iter()
                .map(|value| match value {
                    ProcessListValue::Null => ShowValue::Null,
                    ProcessListValue::Unsigned(value) => ShowValue::Uint64(value),
                    ProcessListValue::Signed(value) => ShowValue::Int64(value),
                    ProcessListValue::Float(value) => ShowValue::Float64(value),
                    ProcessListValue::Text(value) => ShowValue::String(value),
                })
                .collect()
        })
        .collect()
}

/// RAII：临时将 SelectLimit 提至最大值，Drop 时恢复。
struct SelectLimitGuard<'a> {
    runtime: &'a dyn ShowRuntimeContext,
    old: u64,
}

impl Drop for SelectLimitGuard<'_> {
    fn drop(&mut self) {
        self.runtime.SetSelectLimit(self.old);
    }
}

/// SHOW 执行器：缓存全量结果行并按 Chunk 容量分页输出。
pub struct ShowExec {
    pub Tp: ShowStmtType,
    pub DBName: String,
    pub Table: Option<TableName>,
    pub Partition: String,
    pub Column: Option<String>,
    pub IndexName: String,
    pub ResourceGroupName: String,
    pub Flag: i32,
    pub Roles: Vec<String>,
    pub User: Option<UserIdentity>,
    pub Extractor: ShowPredicate,
    pub CountWarningsOrErrors: bool,
    pub Full: bool,
    pub IfNotExists: bool,
    pub GlobalScope: bool,
    pub Extended: bool,
    pub ImportJobID: Option<i64>,
    pub DistributionJobID: Option<i64>,
    pub ImportGroupKey: String,
    result: Option<Vec<Vec<ShowValue>>>,
    cursor: usize,
    runtime: Arc<dyn ShowRuntimeContext>,
}

impl ShowExec {
    /// 按语句类型构造空参数的执行器实例。
    pub fn new(runtime: Arc<dyn ShowRuntimeContext>, statement: ShowStmtType) -> Self {
        Self {
            Tp: statement,
            DBName: String::new(),
            Table: None,
            Partition: String::new(),
            Column: None,
            IndexName: String::new(),
            ResourceGroupName: String::new(),
            Flag: 0,
            Roles: Vec::new(),
            User: None,
            Extractor: ShowPredicate::default(),
            CountWarningsOrErrors: false,
            Full: false,
            IfNotExists: false,
            GlobalScope: false,
            Extended: false,
            ImportJobID: None,
            DistributionJobID: None,
            ImportGroupKey: String::new(),
            result: None,
            cursor: 0,
            runtime,
        }
    }

    /// 将执行器字段打包为运行时请求。
    fn request(&self) -> ShowRequest {
        ShowRequest {
            statement: self.Tp,
            database: self.DBName.clone(),
            table: self.Table.clone(),
            partition: self.Partition.clone(),
            column: self.Column.clone(),
            index_name: self.IndexName.clone(),
            resource_group_name: self.ResourceGroupName.clone(),
            flag: self.Flag,
            roles: self.Roles.clone(),
            user: self.User.clone(),
            extractor: self.Extractor.clone(),
            count_warnings_or_errors: self.CountWarningsOrErrors,
            full: self.Full,
            if_not_exists: self.IfNotExists,
            global_scope: self.GlobalScope,
            extended: self.Extended,
            import_job_id: self.ImportJobID,
            distribution_job_id: self.DistributionJobID,
            import_group_key: self.ImportGroupKey.clone(),
        }
    }

    /// 首次调用触发 `fetchAll`，之后按 Chunk 容量推进游标写出。
    pub fn Next(&mut self, req: &mut chunk::Chunk) -> ShowResult {
        req.GrowAndReset(self.runtime.MaxChunkSize());
        if self.result.is_none() {
            self.result = Some(Vec::new());
            self.fetchAll()?;
        }
        let rows = self.result.as_ref().expect("SHOW result initialized");
        if self.cursor >= rows.len() {
            return Ok(());
        }
        let capacity = req.Capacity().max(1);
        let end = rows.len().min(self.cursor + capacity);
        for row in &rows[self.cursor..end] {
            appendRowToChunk(req, row)?;
        }
        self.cursor = end;
        Ok(())
    }

    /// 临时放开 SelectLimit，按 `Tp` 分发到各 fetchShow* 实现。
    pub fn fetchAll(&mut self) -> ShowResult {
        // 拉取全量行时暂时取消会话 SelectLimit，退出时由 Guard 恢复。
        let old = self.runtime.SelectLimit();
        self.runtime.SetSelectLimit(u64::MAX);
        let runtime = Arc::clone(&self.runtime);
        let _guard = SelectLimitGuard {
            runtime: runtime.as_ref(),
            old,
        };
        match self.Tp {
            ShowStmtType::Charset => self.fetchShowCharset(),
            ShowStmtType::Collation => self.fetchShowCollation(),
            ShowStmtType::Columns => self.fetchShowColumns(),
            ShowStmtType::Config => self.fetchShowClusterConfigs(),
            ShowStmtType::CreateTable => self.fetchShowCreateTable(),
            ShowStmtType::CreateSequence => self.fetchShowCreateSequence(),
            ShowStmtType::CreateUser => self.fetchShowCreateUser(),
            ShowStmtType::CreateView => self.fetchShowCreateView(),
            ShowStmtType::CreateDatabase => self.fetchShowCreateDatabase(),
            ShowStmtType::CreatePlacementPolicy => self.fetchShowCreatePlacementPolicy(),
            ShowStmtType::CreateResourceGroup => self.fetchShowCreateResourceGroup(),
            ShowStmtType::Databases => self.fetchShowDatabases(),
            ShowStmtType::Engines => self.fetchShowEngines(),
            ShowStmtType::Grants => self.fetchShowGrants(),
            ShowStmtType::Index => self.fetchShowIndex(),
            ShowStmtType::ProcedureStatus => self.fetchShowProcedureStatus(),
            ShowStmtType::Status => self.fetchShowStatus(),
            ShowStmtType::Tables => self.fetchShowTables(),
            ShowStmtType::OpenTables => self.fetchShowOpenTables(),
            ShowStmtType::TableStatus => self.fetchShowTableStatus(),
            ShowStmtType::Triggers => self.fetchShowTriggers(),
            ShowStmtType::Variables => self.fetchShowVariables(),
            ShowStmtType::Warnings => self.fetchShowWarnings(false),
            ShowStmtType::Errors => self.fetchShowWarnings(true),
            ShowStmtType::ProcessList => self.fetchShowProcessList(),
            ShowStmtType::Events | ShowStmtType::Profiles => Ok(()),
            ShowStmtType::StatsExtended => {
                Err(errors::New("Extended statistics feature has been removed"))
            }
            ShowStmtType::StatsMeta => self.fetchOperation(ShowOperation::StatsMeta),
            ShowStmtType::StatsHistograms => self.fetchOperation(ShowOperation::StatsHistograms),
            ShowStmtType::StatsBuckets => self.fetchOperation(ShowOperation::StatsBuckets),
            ShowStmtType::StatsTopN => self.fetchOperation(ShowOperation::StatsTopN),
            ShowStmtType::StatsHealthy => self.fetchOperation(ShowOperation::StatsHealthy),
            ShowStmtType::StatsLocked => self.fetchOperation(ShowOperation::StatsLocked),
            ShowStmtType::HistogramsInFlight => {
                self.fetchOperation(ShowOperation::HistogramsInFlight)
            }
            ShowStmtType::ColumnStatsUsage => self.fetchOperation(ShowOperation::ColumnStatsUsage),
            ShowStmtType::Plugins => self.fetchShowPlugins(),
            ShowStmtType::MasterStatus | ShowStmtType::BinlogStatus => self.fetchShowMasterStatus(),
            ShowStmtType::Privileges => self.fetchShowPrivileges(),
            ShowStmtType::Bindings => self.fetchShowBind(),
            ShowStmtType::BindingCacheStatus => self.fetchShowBindingCacheStatus(),
            ShowStmtType::AnalyzeStatus => self.fetchOperation(ShowOperation::AnalyzeStatus),
            ShowStmtType::Regions => self.fetchShowTableRegions(),
            ShowStmtType::Distributions => self.fetchShowDistributions(),
            ShowStmtType::Builtins => self.fetchShowBuiltins(),
            ShowStmtType::Backups => self.fetchOperation(ShowOperation::Backups),
            ShowStmtType::Restores => self.fetchOperation(ShowOperation::Restores),
            ShowStmtType::PlacementLabels => self.fetchOperation(ShowOperation::PlacementLabels),
            ShowStmtType::Placement => self.fetchOperation(ShowOperation::Placement),
            ShowStmtType::PlacementForDatabase => {
                self.fetchOperation(ShowOperation::PlacementForDatabase)
            }
            ShowStmtType::PlacementForTable => {
                self.fetchOperation(ShowOperation::PlacementForTable)
            }
            ShowStmtType::PlacementForPartition => {
                self.fetchOperation(ShowOperation::PlacementForPartition)
            }
            ShowStmtType::SessionStates => self.fetchShowSessionStates(),
            ShowStmtType::ImportJobs => self.fetchShowImportJobs(),
            ShowStmtType::ImportGroups => self.fetchShowImportGroups(),
            ShowStmtType::DistributionJobs => self.fetchShowDistributionJobs(),
            ShowStmtType::Affinity => self.fetchOperation(ShowOperation::Affinity),
        }
    }

    /// 经运行时 `FetchRows` 取数并追加到结果缓存。
    fn fetchOperation(&mut self, operation: ShowOperation) -> ShowResult {
        let rows = self.runtime.FetchRows(operation, &self.request())?;
        self.result_mut().extend(rows);
        Ok(())
    }

    /// 惰性初始化结果行缓冲。
    fn result_mut(&mut self) -> &mut Vec<Vec<ShowValue>> {
        self.result.get_or_insert_with(Vec::new)
    }

    /// SHOW BINDINGS。
    pub fn fetchShowBind(&mut self) -> ShowResult {
        self.fetchOperation(ShowOperation::Bindings)
    }

    /// SHOW BINDING CACHE STATUS。
    pub fn fetchShowBindingCacheStatus(&mut self) -> ShowResult {
        self.fetchOperation(ShowOperation::BindingCacheStatus)
    }

    /// SHOW ENGINES。
    pub fn fetchShowEngines(&mut self) -> ShowResult {
        self.fetchOperation(ShowOperation::Engines)
    }

    /// SHOW DATABASES：可见性过滤、字段/LIKE 过滤，information_schema 置前。
    pub fn fetchShowDatabases(&mut self) -> ShowResult {
        let mut databases = self.runtime.AllSchemaNames()?;
        databases.sort_unstable_by_key(|name| name.to_lowercase());
        // MySQL 兼容：information_schema 固定排在最前。
        moveInfoSchemaToFront(&mut databases);
        let field = self.Extractor.field.as_deref().map(str::to_lowercase);
        for database in databases {
            if !self.runtime.DatabaseVisible(&database)? {
                continue;
            }
            let lower = database.to_lowercase();
            if field.as_ref().is_some_and(|expected| expected != &lower) {
                continue;
            }
            if self
                .Extractor
                .like_pattern
                .as_deref()
                .is_some_and(|pattern| !sqlLike(pattern, &lower))
            {
                continue;
            }
            self.appendRow(vec![database.into()]);
        }
        Ok(())
    }

    /// SHOW PROCESSLIST。
    pub fn fetchShowProcessList(&mut self) -> ShowResult {
        let manager = self.runtime.GetSessionManager();
        let login_user = self.runtime.LoginUserName();
        let rows = FetchShowProcessListRows(
            manager.as_deref(),
            login_user.as_deref(),
            self.runtime.HasProcessPrivilege(),
            self.Full,
        );
        self.result_mut().extend(rows);
        Ok(())
    }

    /// SHOW OPEN TABLES（当前实现为空结果）。
    pub fn fetchShowOpenTables(&mut self) -> ShowResult {
        Ok(())
    }

    /// 将 `TableKind` 映射为 MySQL 兼容的 TableType 字符串。
    pub fn getTableType(&self, table: &TableInfo) -> &'static str {
        match table.kind {
            TableKind::View => "VIEW",
            TableKind::Sequence => "SEQUENCE",
            TableKind::SystemView => "SYSTEM VIEW",
            TableKind::BaseTable => "BASE TABLE",
        }
    }

    /// 按精确表名取展示信息；临时表返回空。
    pub fn fetchShowInfoByName(&self, name: &str) -> ShowResult<Vec<showInfo>> {
        let Some(table) = self.runtime.TableByName(&self.DBName, name)? else {
            return Ok(Vec::new());
        };
        if table.temporary {
            return Ok(Vec::new());
        }
        Ok(vec![showInfo {
            Name: table.name.clone(),
            TableType: self.getTableType(&table).to_owned(),
        }])
    }

    /// 非 FULL：仅表名。
    pub fn fetchShowSimpleTables(&self) -> ShowResult<Vec<showInfo>> {
        Ok(self
            .runtime
            .Tables(&self.DBName, false)?
            .into_iter()
            .filter(|table| !table.temporary)
            .map(|table| showInfo {
                Name: table.name,
                TableType: String::new(),
            })
            .collect())
    }

    /// FULL：表名加 TableType。
    pub fn fetchShowFullTables(&self) -> ShowResult<Vec<showInfo>> {
        Ok(self
            .runtime
            .Tables(&self.DBName, true)?
            .into_iter()
            .filter(|table| !table.temporary)
            .map(|table| showInfo {
                TableType: self.getTableType(&table).to_owned(),
                Name: table.name,
            })
            .collect())
    }

    /// SHOW TABLES：校验库可见性/存在性，再过滤写出。
    pub fn fetchShowTables(&mut self) -> ShowResult {
        if !self.runtime.DatabaseVisible(&self.DBName)? {
            return Err(self.dbAccessDenied());
        }
        if !self.runtime.SchemaExists(&self.DBName)? {
            return Err(errors::New(format!("Unknown database '{}'", self.DBName)));
        }
        let field = self.Extractor.field.as_deref().map(str::to_lowercase);
        let mut tables = if let Some(name) = field.as_deref() {
            self.fetchShowInfoByName(name)?
        } else if self.Full {
            self.fetchShowFullTables()?
        } else {
            self.fetchShowSimpleTables()?
        };
        tables.sort_unstable_by_key(|table| table.Name.to_lowercase());
        for table in tables {
            if !self.runtime.TableVisible(&self.DBName, &table.Name)? {
                continue;
            }
            let lower = table.Name.to_lowercase();
            if field.as_ref().is_some_and(|expected| expected != &lower) {
                continue;
            }
            if self
                .Extractor
                .like_pattern
                .as_deref()
                .is_some_and(|pattern| !sqlLike(pattern, &lower))
            {
                continue;
            }
            if self.Full {
                self.appendRow(vec![table.Name.into(), table.TableType.into()]);
            } else {
                self.appendRow(vec![table.Name.into()]);
            }
        }
        Ok(())
    }

    /// SHOW TABLE STATUS。
    pub fn fetchShowTableStatus(&mut self) -> ShowResult {
        self.fetchOperation(ShowOperation::TableStatus)
    }

    /// SHOW COLUMNS：先解析目标表。
    pub fn fetchShowColumns(&mut self) -> ShowResult {
        self.getTable()?;
        self.fetchOperation(ShowOperation::Columns)
    }

    /// SHOW INDEX：先解析目标表。
    pub fn fetchShowIndex(&mut self) -> ShowResult {
        self.getTable()?;
        self.fetchOperation(ShowOperation::Index)
    }

    /// SHOW CHARSET。
    pub fn fetchShowCharset(&mut self) -> ShowResult {
        self.fetchOperation(ShowOperation::Charset)
    }

    /// SHOW MASTER STATUS / BINLOG STATUS。
    pub fn fetchShowMasterStatus(&mut self) -> ShowResult {
        self.fetchOperation(ShowOperation::MasterStatus)
    }

    /// SHOW VARIABLES。
    pub fn fetchShowVariables(&mut self) -> ShowResult {
        self.fetchOperation(ShowOperation::Variables)
    }

    /// SHOW STATUS。
    pub fn fetchShowStatus(&mut self) -> ShowResult {
        self.fetchOperation(ShowOperation::Status)
    }

    /// SHOW CREATE SEQUENCE。
    pub fn fetchShowCreateSequence(&mut self) -> ShowResult {
        self.fetchOperation(ShowOperation::CreateSequence)
    }

    /// SHOW CONFIG（集群配置）。
    pub fn fetchShowClusterConfigs(&mut self) -> ShowResult {
        self.fetchOperation(ShowOperation::ClusterConfigs)
    }

    /// SHOW CREATE TABLE。
    pub fn fetchShowCreateTable(&mut self) -> ShowResult {
        self.fetchOperation(ShowOperation::CreateTable)
    }

    /// SHOW CREATE VIEW。
    pub fn fetchShowCreateView(&mut self) -> ShowResult {
        self.fetchOperation(ShowOperation::CreateView)
    }

    /// SHOW CREATE DATABASE。
    pub fn fetchShowCreateDatabase(&mut self) -> ShowResult {
        self.fetchOperation(ShowOperation::CreateDatabase)
    }

    /// SHOW CREATE PLACEMENT POLICY。
    pub fn fetchShowCreatePlacementPolicy(&mut self) -> ShowResult {
        self.fetchOperation(ShowOperation::CreatePlacementPolicy)
    }

    /// SHOW CREATE RESOURCE GROUP。
    pub fn fetchShowCreateResourceGroup(&mut self) -> ShowResult {
        self.fetchOperation(ShowOperation::CreateResourceGroup)
    }

    /// SHOW COLLATION。
    pub fn fetchShowCollation(&mut self) -> ShowResult {
        self.fetchOperation(ShowOperation::Collation)
    }

    /// SHOW CREATE USER。
    pub fn fetchShowCreateUser(&mut self) -> ShowResult {
        self.fetchOperation(ShowOperation::CreateUser)
    }

    /// SHOW GRANTS。
    pub fn fetchShowGrants(&mut self) -> ShowResult {
        self.fetchOperation(ShowOperation::Grants)
    }

    /// SHOW PRIVILEGES。
    pub fn fetchShowPrivileges(&mut self) -> ShowResult {
        self.fetchOperation(ShowOperation::Privileges)
    }

    /// SHOW TRIGGERS。
    pub fn fetchShowTriggers(&mut self) -> ShowResult {
        self.fetchOperation(ShowOperation::Triggers)
    }

    /// SHOW PROCEDURE STATUS。
    pub fn fetchShowProcedureStatus(&mut self) -> ShowResult {
        self.fetchOperation(ShowOperation::ProcedureStatus)
    }

    /// SHOW PLUGINS。
    pub fn fetchShowPlugins(&mut self) -> ShowResult {
        self.fetchOperation(ShowOperation::Plugins)
    }

    /// SHOW WARNINGS / ERRORS。
    pub fn fetchShowWarnings(&mut self, err_only: bool) -> ShowResult {
        self.fetchOperation(if err_only {
            ShowOperation::Errors
        } else {
            ShowOperation::Warnings
        })
    }

    /// 解析并校验当前请求中的目标表。
    pub fn getTable(&self) -> ShowResult<TableInfo> {
        let table = self
            .Table
            .as_ref()
            .ok_or_else(|| errors::New("SHOW requires a table"))?;
        if !self.runtime.TableVisible(&table.schema, &table.name)? {
            return Err(self.tableAccessDenied("SELECT", &table.name));
        }
        self.runtime
            .TableByName(&table.schema, &table.name)?
            .ok_or_else(|| {
                errors::New(format!(
                    "Table '{}.{}' doesn't exist",
                    table.schema, table.name
                ))
            })
    }

    /// 构造库级 Access denied 错误。
    pub fn dbAccessDenied(&self) -> errors::SharedError {
        let user = self.User.as_ref().map_or("", |user| user.username.as_str());
        let host = self.User.as_ref().map_or("", |user| user.hostname.as_str());
        errors::New(format!(
            "Access denied for user '{user}'@'{host}' to database '{}'",
            self.DBName
        ))
    }

    /// 构造表级命令拒绝错误。
    pub fn tableAccessDenied(&self, access: &str, table: &str) -> errors::SharedError {
        let user = self.User.as_ref().map_or("", |user| user.username.as_str());
        let host = self.User.as_ref().map_or("", |user| user.hostname.as_str());
        errors::New(format!(
            "{access} command denied to user '{user}'@'{host}' for table '{table}'"
        ))
    }

    /// 追加一行到结果缓存。
    pub fn appendRow(&mut self, row: Vec<ShowValue>) {
        self.result_mut().push(row);
    }

    /// SHOW TABLE DISTRIBUTIONS。
    pub fn fetchShowDistributions(&mut self) -> ShowResult {
        self.fetchOperation(ShowOperation::Distributions)
    }

    /// SHOW TABLE REGIONS：按表或索引取 Region 并附调度信息。
    pub fn fetchShowTableRegions(&mut self) -> ShowResult {
        let table = self
            .Table
            .clone()
            .ok_or_else(|| errors::New("SHOW REGIONS requires a table"))?;
        let physical_ids = self.getTable()?.id;
        // 无索引名时按表 Region，否则按索引 Region。
        let regions = if self.IndexName.is_empty() {
            getTableRegions(self.runtime.as_ref(), &table, &[physical_ids])?
        } else {
            getTableIndexRegions(
                self.runtime.as_ref(),
                &table,
                &self.IndexName,
                &[physical_ids],
            )?
        };
        let regions = self.fetchSchedulingInfo(&regions, &table)?;
        self.fillRegionsToChunk(&regions);
        Ok(())
    }

    /// 为 Region 列表补充调度约束与状态。
    pub fn fetchSchedulingInfo(
        &self,
        regions: &[regionMeta],
        table: &TableName,
    ) -> ShowResult<Vec<showTableRegionRowItem>> {
        self.runtime.SchedulingInfo(regions, table)
    }

    /// 将分布统计编码为结果行。
    pub fn fillDistributionsToChunk(
        &mut self,
        partition_name: &str,
        distributions: &[RegionDistribution],
    ) {
        for distribution in distributions {
            self.appendRow(vec![
                partition_name.into(),
                ShowValue::Uint64(distribution.store_id),
                distribution.engine_type.clone().into(),
                ShowValue::Int64(distribution.region_leader_count),
                ShowValue::Int64(distribution.region_peer_count),
                ShowValue::Uint64(distribution.region_write_bytes),
                ShowValue::Uint64(distribution.region_write_keys),
                ShowValue::Uint64(distribution.region_write_query),
                ShowValue::Uint64(distribution.region_leader_read_bytes),
                ShowValue::Uint64(distribution.region_leader_read_keys),
                ShowValue::Uint64(distribution.region_leader_read_query),
                ShowValue::Uint64(distribution.region_peer_read_bytes),
                ShowValue::Uint64(distribution.region_peer_read_keys),
                ShowValue::Uint64(distribution.region_peer_read_query),
            ]);
        }
    }

    /// 将 Region 行编码为 SHOW TABLE REGIONS 列布局。
    pub fn fillRegionsToChunk(&mut self, regions: &[showTableRegionRowItem]) {
        for item in regions {
            let peers = item
                .regionMeta
                .peers
                .iter()
                .map(|peer| peer.id.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            self.appendRow(vec![
                ShowValue::Uint64(item.regionMeta.id),
                item.regionMeta.start.clone().into(),
                item.regionMeta.end.clone().into(),
                ShowValue::Uint64(item.regionMeta.leader_id),
                ShowValue::Uint64(item.regionMeta.store_id),
                peers.into(),
                ShowValue::Int64(i64::from(item.regionMeta.scattering)),
                ShowValue::Uint64(item.regionMeta.written_bytes),
                ShowValue::Uint64(item.regionMeta.read_bytes),
                ShowValue::Int64(item.regionMeta.approximate_size),
                ShowValue::Int64(item.regionMeta.approximate_keys),
                item.schedulingConstraints.clone().into(),
                item.schedulingState.clone().into(),
            ]);
        }
    }

    /// SHOW BUILTINS。
    pub fn fetchShowBuiltins(&mut self) -> ShowResult {
        self.fetchOperation(ShowOperation::Builtins)
    }

    /// SHOW SESSION STATES。
    pub fn fetchShowSessionStates(&mut self) -> ShowResult {
        self.fetchOperation(ShowOperation::SessionStates)
    }

    /// SHOW DISTRIBUTION JOBS：可按 job id 过滤。
    pub fn fetchShowDistributionJobs(&mut self) -> ShowResult {
        let request = self.request();
        let mut jobs = self.runtime.DistributionJobs(&request)?;
        jobs.sort_unstable_by_key(|job| job.job_id);
        for job in jobs {
            if request
                .distribution_job_id
                .is_some_and(|wanted| wanted < 0 || wanted as u64 != job.job_id)
            {
                continue;
            }
            self.result_mut().push(fillDistributionJobToChunk(&job)?);
        }
        Ok(())
    }

    /// SHOW IMPORT GROUPS：按 group_key 聚合作业计数。
    pub fn fetchShowImportGroups(&mut self) -> ShowResult {
        let request = self.request();
        let jobs = self.runtime.ImportJobs(&request)?;
        // 按 group_key 聚合各状态计数，并跟踪最早创建/最晚更新时间。
        let mut groups = BTreeMap::<String, groupInfo>::new();
        for job in jobs {
            // Go GetJobsByGroupKey excludes jobs without a group key even when
            // the caller does not request a named group.
            if job.group_key.is_empty() {
                continue;
            }
            if !request.import_group_key.is_empty() && request.import_group_key != job.group_key {
                continue;
            }
            let runtime = if job.status == "running" {
                self.runtime.ImportRuntime(job.id)?
            } else {
                None
            };
            let update_time = runtime
                .as_ref()
                .and_then(|info| info.update_time)
                .or(job.update_time);
            let group = groups
                .entry(job.group_key.clone())
                .or_insert_with(|| groupInfo {
                    groupKey: job.group_key.clone(),
                    ..groupInfo::default()
                });
            group.jobCount += 1;
            match job.status.as_str() {
                "pending" => group.pending += 1,
                "running" => group.running += 1,
                "finished" => group.completed += 1,
                "failed" => group.failed += 1,
                "cancelled" => group.canceled += 1,
                _ => {}
            }
            group.createTime = earliest(group.createTime, job.create_time);
            group.updateTime = latest(group.updateTime, update_time);
        }
        for group in groups.into_values() {
            self.appendRow(vec![
                group.groupKey.into(),
                ShowValue::Int64(group.jobCount),
                ShowValue::Int64(group.pending),
                ShowValue::Int64(group.running),
                ShowValue::Int64(group.completed),
                ShowValue::Int64(group.failed),
                ShowValue::Int64(group.canceled),
                timeValue(group.createTime),
                timeValue(group.updateTime),
            ]);
        }
        Ok(())
    }

    /// SHOW IMPORT JOBS：合并运行时进度与 awaiting-resolution 状态。
    pub fn fetchShowImportJobs(&mut self) -> ShowResult {
        let request = self.request();
        let mut jobs = self.runtime.ImportJobs(&request)?;
        jobs.sort_unstable_by_key(|job| job.id);
        for mut job in jobs {
            if request.import_job_id.is_some_and(|wanted| wanted != job.id) {
                continue;
            }
            let runtime = if job.status == "running" {
                self.runtime.ImportRuntime(job.id)?
            } else {
                None
            };
            // 运行时若存在待解决冲突错误，覆盖展示状态。
            if let Some(message) = runtime
                .as_ref()
                .and_then(|runtime| runtime.awaiting_resolution_error.as_ref())
            {
                job.status = "awaiting-resolution".to_owned();
                job.error_message = message.clone();
            }
            handleImportJobInfo(self.result_mut(), &job, runtime.as_ref());
        }
        Ok(())
    }
}

/// 视图依赖检查用的表引用。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableReference {
    pub schema: String,
    pub table: String,
}

/// 遍历表引用时检查当前用户是否可见；任一不可见则 `ok=false`。
pub struct visibleChecker {
    pub defaultDB: String,
    pub ok: bool,
    runtime: Arc<dyn ShowRuntimeContext>,
}

impl visibleChecker {
    /// 以默认库与运行时构造检查器。
    pub fn new(default_database: String, runtime: Arc<dyn ShowRuntimeContext>) -> Self {
        Self {
            defaultDB: default_database,
            ok: true,
            runtime,
        }
    }

    /// 进入表节点：存在但不可见时清除 `ok`。
    pub fn Enter(&mut self, table: &TableReference) -> ShowResult<bool> {
        let schema = if table.schema.is_empty() {
            &self.defaultDB
        } else {
            &table.schema
        };
        if self.runtime.TableByName(schema, &table.table)?.is_none() {
            return Ok(true);
        }
        if !self.runtime.TableVisible(schema, &table.table)? {
            self.ok = false;
        }
        Ok(true)
    }

    /// 离开表节点（透传）。
    pub fn Leave(&self, table: TableReference) -> (TableReference, bool) {
        (table, true)
    }
}

/// 将 `information_schema` 移到库名列表最前。
pub fn moveInfoSchemaToFront(databases: &mut [String]) {
    if let Some(position) = databases
        .iter()
        .position(|name| name.eq_ignore_ascii_case("information_schema"))
    {
        databases[..=position].rotate_right(1);
    }
}

/// 返回字符集的默认校对规则名。
pub fn getDefaultCollate(charset_name: &str) -> &'static str {
    match charset_name.to_ascii_lowercase().as_str() {
        "utf8mb4" => "utf8mb4_bin",
        "utf8" | "utf8mb3" => "utf8_bin",
        "ascii" => "ascii_bin",
        "latin1" => "latin1_bin",
        "binary" => "binary",
        _ => "",
    }
}

/// 对外入口：构造 SHOW CREATE TABLE 文本。
pub fn ConstructResultOfShowCreateTable(
    formatter: &dyn ShowCreateFormatter,
    table: &TableInfo,
) -> ShowResult<String> {
    constructResultOfShowCreateTable(formatter, None, table)
}

/// 内部实现：可选带库名的 CREATE TABLE 格式化。
pub fn constructResultOfShowCreateTable(
    formatter: &dyn ShowCreateFormatter,
    database: Option<&str>,
    table: &TableInfo,
) -> ShowResult<String> {
    formatter.FormatCreateTable(&CreateTableInput {
        database: database.map(str::to_owned),
        table: table.clone(),
        if_not_exists: false,
    })
}

/// 构造 SHOW CREATE SEQUENCE 文本。
pub fn ConstructResultOfShowCreateSequence(
    formatter: &dyn ShowCreateFormatter,
    table: &TableInfo,
) -> ShowResult<String> {
    formatter.FormatCreateSequence(&CreateTableInput {
        database: None,
        table: table.clone(),
        if_not_exists: false,
    })
}

/// 视图场景下构造 CREATE VIEW 文本。
pub fn fetchShowCreateTable4View(
    formatter: &dyn ShowCreateFormatter,
    table: &TableInfo,
) -> ShowResult<String> {
    formatter.FormatCreateView(&CreateTableInput {
        database: None,
        table: table.clone(),
        if_not_exists: false,
    })
}

/// 拼装 CREATE DATABASE 语句（含可选字符集/校对/Placement）。
pub fn ConstructResultOfShowCreateDatabase(input: &CreateDatabaseInput) -> ShowResult<String> {
    if input.name.is_empty() {
        return Err(errors::New("database name must not be empty"));
    }
    let mut result = String::from("CREATE DATABASE ");
    if input.if_not_exists {
        // Go emits IF NOT EXISTS as ordinary syntax; the version comment is
        // used only for the optional charset clause below.
        result.push_str("IF NOT EXISTS ");
    }
    write!(result, "`{}`", escapeIdentifier(&input.name)).expect("writing to String cannot fail");
    if !input.charset.is_empty() {
        write!(result, " /*!40100 DEFAULT CHARACTER SET {}", input.charset)
            .expect("writing to String cannot fail");
        if !input.collation.is_empty() && getDefaultCollate(&input.charset) != input.collation {
            write!(result, " COLLATE {}", input.collation).expect("writing to String cannot fail");
        }
        result.push_str(" */");
    }
    if let Some(policy) = input.placement_policy.as_deref() {
        write!(
            result,
            " /*T![placement] PLACEMENT POLICY=`{}` */",
            escapeIdentifier(policy)
        )
        .expect("writing to String cannot fail");
    }
    Ok(result)
}

/// 拼装 CREATE PLACEMENT POLICY 语句。
pub fn ConstructResultOfShowCreatePlacementPolicy(policy: &PlacementPolicyInfo) -> String {
    format!(
        "CREATE PLACEMENT POLICY `{}` {}",
        escapeIdentifier(&policy.name),
        policy.settings
    )
}

/// 拼装 CREATE RESOURCE GROUP 语句。
pub fn constructResultOfShowCreateResourceGroup(group: &ResourceGroupInfo) -> String {
    format!(
        "CREATE RESOURCE GROUP `{}` {}",
        escapeIdentifier(&group.name),
        group.settings
    )
}

/// 判断是否 utf8mb4，以及是否为其默认校对。
pub fn isUTF8MB4AndDefaultCollation(charset: &str, collation: &str) -> ShowResult<(bool, bool)> {
    if charset.is_empty() || collation.is_empty() {
        return Err(errors::New("charset and collation must not be empty"));
    }
    let is_utf8mb4 = charset.eq_ignore_ascii_case("utf8mb4");
    Ok((
        is_utf8mb4,
        is_utf8mb4 && collation.eq_ignore_ascii_case(getDefaultCollate(charset)),
    ))
}

/// 拉取表 Region 列表并按 Region ID 去重。
pub fn getTableRegions(
    runtime: &dyn ShowRuntimeContext,
    table: &TableName,
    physical_ids: &[i64],
) -> ShowResult<Vec<regionMeta>> {
    deduplicateRegions(runtime.TableRegions(table, physical_ids)?)
}

/// 拉取索引 Region 列表并按 Region ID 去重。
pub fn getTableIndexRegions(
    runtime: &dyn ShowRuntimeContext,
    table: &TableName,
    index_name: &str,
    physical_ids: &[i64],
) -> ShowResult<Vec<regionMeta>> {
    deduplicateRegions(runtime.IndexRegions(table, index_name, physical_ids)?)
}

/// 将单个 IMPORT 作业（含可选运行时）编码为一行。
pub fn FillOneImportJobInfo(
    result: &mut Vec<Vec<ShowValue>>,
    info: &ImportJobInfo,
    run_info: Option<&ImportRuntimeInfo>,
) {
    let full_table_name = format!(
        "`{}`.`{}`",
        escapeIdentifier(&info.table_schema),
        escapeIdentifier(&info.table_name)
    );
    let source_size = info.source_file_size.map_or_else(
        || ShowValue::String("N/A".to_owned()),
        |size| ShowValue::String(formatBytes(size)),
    );
    let imported_rows = run_info
        .map(|runtime| ShowValue::Uint64(runtime.import_rows))
        .or_else(|| {
            info.IsSuccess()
                .then_some(ShowValue::Uint64(info.imported_rows))
        })
        .unwrap_or(ShowValue::Null);
    let message = if info.IsSuccess() {
        let mut messages = Vec::new();
        if info.conflict_row_count > 0 {
            messages.push(format!("{} conflicted rows.", info.conflict_row_count));
        }
        if info.too_many_conflicts {
            messages.push("Too many conflicted rows, checksum skipped.".to_owned());
        }
        messages.join(" ")
    } else {
        info.error_message.clone()
    };
    let update_time = match run_info {
        Some(runtime) => runtime.update_time.or(info.update_time),
        None => info.end_time,
    };
    let mut row = vec![
        ShowValue::Int64(info.id),
        if info.group_key.is_empty() {
            ShowValue::Null
        } else {
            info.group_key.clone().into()
        },
        info.file_location.clone().into(),
        full_table_name.into(),
        ShowValue::Int64(info.table_id),
        info.step.clone().into(),
        info.status.clone().into(),
        source_size,
        imported_rows,
        message.into(),
        timeValue(info.create_time),
        timeValue(info.start_time),
        timeValue(info.end_time),
        info.created_by.clone().into(),
        timeValue(update_time),
    ];
    if let Some(runtime) = run_info {
        row.extend([
            runtime.step.clone().into(),
            runtime.processed_size.clone().into(),
            runtime.total_size.clone().into(),
            runtime.percent.clone().into(),
            runtime.speed.clone().into(),
            runtime.eta.clone().into(),
        ]);
    } else {
        row.extend(std::iter::repeat_n(ShowValue::Null, 6));
    }
    result.push(row);
}

/// 处理并写入一条 IMPORT 作业信息（委托 `FillOneImportJobInfo`）。
pub fn handleImportJobInfo(
    result: &mut Vec<Vec<ShowValue>>,
    info: &ImportJobInfo,
    run_info: Option<&ImportRuntimeInfo>,
) {
    FillOneImportJobInfo(result, info, run_info);
}

#[cfg(test)]
#[path = "show/import_groups_test.rs"]
mod import_groups_test;

/// 将分布作业编码为结果行；alias 须为 schema.table.partition 三段。
pub fn fillDistributionJobToChunk(job: &DistributionJob) -> ShowResult<Vec<ShowValue>> {
    let alias = job.alias.split('.').collect::<Vec<_>>();
    if alias.len() != 3 {
        return Err(errors::New(format!("alias:{} is invalid", job.alias)));
    }
    Ok(vec![
        ShowValue::Uint64(job.job_id),
        alias[0].into(),
        alias[1].into(),
        if alias[2].is_empty() {
            ShowValue::Null
        } else {
            alias[2].into()
        },
        job.engine.clone().into(),
        job.rule.clone().into(),
        job.status.clone().into(),
        formatDuration(job.timeout).into(),
        timeValue(job.create),
        timeValue(job.start),
        timeValue(job.finish),
    ])
}

/// 若对象是视图，则在系统会话中补齐列类型信息。
pub fn tryFillViewColumnType(
    runtime: &dyn ShowRuntimeContext,
    database: &str,
    table: &TableInfo,
) -> ShowResult {
    if table.kind != TableKind::View {
        return Ok(());
    }
    runWithSystemSession(runtime, |system| system.FillViewColumnType(database, table))
}

/// 在系统会话上下文中执行回调。
pub fn runWithSystemSession(
    runtime: &dyn ShowRuntimeContext,
    mut action: impl FnMut(&dyn ShowRuntimeContext) -> ShowResult,
) -> ShowResult {
    runtime.RunWithSystemSession(&mut action)
}

/// 格式化 SPLIT 相关表达式文本。
pub fn formatSplitValue(runtime: &dyn ShowRuntimeContext, value: &str) -> ShowResult<String> {
    runtime.FormatSplitExpression(value)
}

/// 将一行 `ShowValue` 按列追加到输出 Chunk。
fn appendRowToChunk(result: &mut chunk::Chunk, row: &[ShowValue]) -> ShowResult {
    if row.len() != result.NumCols() {
        return Err(errors::New(format!(
            "SHOW row has {} columns, output chunk has {}",
            row.len(),
            result.NumCols()
        )));
    }
    for (column, value) in row.iter().enumerate() {
        match value {
            ShowValue::Null => result.AppendNull(column),
            ShowValue::Int64(value) => result.AppendInt64(column, *value),
            ShowValue::Uint64(value) => result.AppendUint64(column, *value),
            ShowValue::Float64(value) => result.AppendFloat64(column, *value),
            ShowValue::String(value) => result.AppendString(column, value),
            ShowValue::Bytes(value) => result.AppendBytes(column, value),
        }
    }
    Ok(())
}

/// 按 Region ID 去重，保留首次出现顺序。
fn deduplicateRegions(regions: Vec<regionMeta>) -> ShowResult<Vec<regionMeta>> {
    let mut seen = HashSet::with_capacity(regions.len());
    let mut unique = Vec::with_capacity(regions.len());
    for region in regions {
        if seen.insert(region.id) {
            unique.push(region);
        }
    }
    Ok(unique)
}

/// SQL LIKE 匹配（大小写不敏感，支持 `\`、`_`、`%`）。
fn sqlLike(pattern: &str, value: &str) -> bool {
    let pattern = pattern.to_lowercase().into_bytes();
    let value = value.to_lowercase().into_bytes();
    // DP：pattern 逐字符推进，维护 value 前缀可达性。
    let mut state = vec![false; value.len() + 1];
    state[0] = true;
    let mut escaped = false;
    for token in pattern {
        let mut next = vec![false; value.len() + 1];
        if escaped {
            for index in 0..value.len() {
                next[index + 1] = state[index] && value[index] == token;
            }
            escaped = false;
        } else {
            match token {
                b'\\' => {
                    escaped = true;
                    next = state;
                }
                b'_' => {
                    next[1..(value.len() + 1)].copy_from_slice(&state[..value.len()]);
                }
                b'%' => {
                    next[0] = state[0];
                    for index in 0..value.len() {
                        next[index + 1] = state[index + 1] || next[index];
                    }
                }
                literal => {
                    for index in 0..value.len() {
                        next[index + 1] = state[index] && value[index] == literal;
                    }
                }
            }
        }
        state = next;
    }
    state[value.len()]
}

/// 取两个可选时间的较早者。
fn earliest(left: Option<SystemTime>, right: Option<SystemTime>) -> Option<SystemTime> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (left, right) => left.or(right),
    }
}

/// 取两个可选时间的较晚者。
fn latest(left: Option<SystemTime>, right: Option<SystemTime>) -> Option<SystemTime> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (left, right) => left.or(right),
    }
}

/// 将 `SystemTime` 格式化为秒.纳秒字符串，缺失则为 NULL。
fn timeValue(time: Option<SystemTime>) -> ShowValue {
    time.map_or(ShowValue::Null, |time| {
        let value = time
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
        ShowValue::String(format!("{}.{:09}", value.as_secs(), value.subsec_nanos()))
    })
}

/// 人类可读字节大小（B/KiB/...）。
fn formatBytes(size: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut value = size as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{}{}", size, UNITS[unit])
    } else {
        format!("{value:.2}{}", UNITS[unit])
    }
}

/// 将 Duration 格式化为带可选小数的秒字符串。
fn formatDuration(duration: Duration) -> String {
    if duration.as_nanos() == 0 {
        return "0s".to_owned();
    }
    if duration.subsec_nanos() == 0 {
        return format!("{}s", duration.as_secs());
    }
    format!("{}.{:09}s", duration.as_secs(), duration.subsec_nanos())
        .trim_end_matches('0')
        .trim_end_matches('s')
        .to_owned()
        + "s"
}

/// 反引号标识符转义（`` ` `` → `` `` ``）。
fn escapeIdentifier(identifier: &str) -> String {
    identifier.replace('`', "``")
}
