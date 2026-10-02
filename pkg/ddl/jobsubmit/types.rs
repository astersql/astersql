// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// Job Submit 公共类型：错误、Job/JobArgs、Session 抽象与 SubmitOptions。
//
// 这些类型描述一次 DDL 任务从构建到写入系统表所需的数据结构，
// 以及与会话、系统表、BDR 策略、DDL owner 通知相关的依赖注入接口。

use std::fmt;
use std::sync::Arc;

use astersql_meta_model::group_3 as model;
use serde_json::{Value, json};

/// 提交路径错误分类。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    /// 可重试错误（如瞬时冲突）。
    Retryable,
    /// 写冲突（悲观锁/事务冲突）。
    WriteConflict,
    /// 参数或状态非法。
    Invalid,
    /// 存储层错误。
    Storage,
}

/// 提交路径统一错误类型。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
}

impl Error {
    /// 构造 Invalid 类错误。
    pub fn invalid(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Invalid,
            message: message.into(),
        }
    }
    /// 构造 Retryable 类错误。
    pub fn retryable(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Retryable,
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

/// DDL Job 类型枚举（与系统表中的 type 码对应）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobType {
    AddIndex,
    AddPrimaryKey,
    ModifyColumn,
    CreateView,
    CreateSequence,
    CreateTable,
    CreateTables,
    CreateSchema,
    CreateResourceGroup,
    AlterTablePartitioning,
    TruncateTablePartition,
    AddTablePartition,
    ReorganizePartition,
    RemovePartitioning,
    TruncateTable,
    RenameTables,
    RenameTable,
    ExchangeTablePartition,
    /// 一次语句包含多个子变更。
    MultiSchemaChange,
    AlterTableMode,
    /// 未建模的其它类型，直接携带原始 type 码。
    Other(i64),
}

impl JobType {
    /// 返回写入系统表的数值 type 码。
    pub fn code(self) -> i64 {
        match self {
            Self::AddIndex => 7,
            Self::AddPrimaryKey => 32,
            Self::ModifyColumn => 12,
            Self::CreateView => 21,
            Self::CreateSequence => 34,
            Self::CreateTable => 3,
            Self::CreateTables => 60,
            Self::CreateSchema => 1,
            Self::CreateResourceGroup => 68,
            Self::AlterTablePartitioning => 71,
            Self::TruncateTablePartition => 23,
            Self::AddTablePartition => 19,
            Self::ReorganizePartition => 64,
            Self::RemovePartitioning => 72,
            Self::TruncateTable => 11,
            Self::RenameTables => 47,
            Self::RenameTable => 14,
            Self::ExchangeTablePartition => 42,
            Self::MultiSchemaChange => 61,
            Self::AlterTableMode => 75,
            Self::Other(value) => value,
        }
    }
}

/// Job 在提交侧关心的状态子集。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum JobState {
    #[default]
    None,
    /// 已入队等待 owner 调度。
    Queueing,
    /// 正在切换到暂停状态（升级期间系统暂停用户 DDL）。
    Pausing,
    /// 暂停（如升级期间）。
    Paused,
}

/// 单个分区定义，提交时主要关心分区 ID。
#[derive(Clone, Debug, Default)]
pub struct PartitionDefinition {
    pub id: i64,
}

/// 分区信息：定义列表，以及部分操作需要的新表 ID。
#[derive(Clone, Debug, Default)]
pub struct PartitionInfo {
    pub definitions: Vec<PartitionDefinition>,
    pub new_table_id: i64,
}

/// 表元信息（提交侧精简版）。
#[derive(Clone, Debug, Default)]
pub struct TableInfo {
    pub id: i64,
    pub partitions: Option<PartitionInfo>,
}

/// 库（schema/database）元信息。
#[derive(Clone, Debug, Default)]
pub struct DatabaseInfo {
    pub id: i64,
}

/// 资源组（Resource Group）元信息。
#[derive(Clone, Debug, Default)]
pub struct ResourceGroupInfo {
    pub id: i64,
}

/// 批量重命名中的单表映射信息。
#[derive(Clone, Debug, Default)]
pub struct RenameTableInfo {
    pub old_schema_id: i64,
    pub new_schema_id: i64,
    pub table_id: i64,
}

/// Job 参数载荷：按 DDL 种类携带不同结构。
#[derive(Clone, Debug)]
pub enum JobArgs {
    None,
    CreateTable {
        table: TableInfo,
    },
    BatchCreateTable {
        tables: Vec<TableInfo>,
    },
    CreateSchema {
        database: DatabaseInfo,
    },
    ResourceGroup {
        group: ResourceGroupInfo,
    },
    TablePartition {
        partition: PartitionInfo,
    },
    TruncateTable {
        old_partition_ids: Vec<i64>,
        new_table_id: i64,
        new_partition_ids: Vec<i64>,
    },
    RenameTables {
        tables: Vec<RenameTableInfo>,
    },
    RenameTable {
        old_schema_id: i64,
    },
    ExchangePartition {
        partition_schema_id: i64,
        partition_table_id: i64,
    },
    /// 不透明字节载荷（未解析类型）。
    Opaque(Vec<u8>),
}

impl Default for JobArgs {
    fn default() -> Self {
        Self::None
    }
}

/// MultiSchemaChange 中的子任务。
#[derive(Clone, Debug, Default)]
pub struct SubJob {
    pub job_type: Option<JobType>,
    pub args: JobArgs,
    pub state: JobState,
    /// ModifyColumn 子任务是否需要回填重组。
    pub need_reorg: bool,
    /// 已编码的子任务参数，插入系统表前填充。
    pub encoded_args: Vec<u8>,
}

/// DDL Job 主体：标识、类型、状态及涉及的 schema/table。
#[derive(Clone, Debug)]
pub struct Job {
    pub id: i64,
    pub version: i64,
    pub schema_id: i64,
    pub table_id: i64,
    pub schema_name: String,
    pub table_name: String,
    pub job_type: JobType,
    pub query: String,
    pub cdc_write_source: u64,
    pub sql_mode: u64,
    /// Complete reorganization configuration captured from the submitting session.
    pub reorg_meta: Option<Arc<model::DDLReorgMeta>>,
    pub session_vars: std::collections::HashMap<String, String>,
    /// 任务开始时间戳（start_ts，事务开始 TS）。
    pub start_ts: u64,
    /// BDR（双向复制）角色名。
    pub bdr_role: String,
    pub state: JobState,
    pub trace_info_present: bool,
    /// Go Job.BinlogInfo 的非空状态；AlterTableMode 必须初始化该字段。
    pub binlog_info_present: bool,
    /// ModifyColumn 是否需要数据回填重组。
    pub need_reorg: bool,
    /// 涉及的 (schema_name, table_name) 列表，用于冲突检测等。
    pub involving_schemas: Vec<(String, String)>,
    pub sub_jobs: Vec<SubJob>,
    /// 是否标记为升级期系统操作。
    pub admin_operator_system: bool,
}

impl Default for Job {
    fn default() -> Self {
        Self {
            id: 0,
            version: 0,
            schema_id: 0,
            table_id: 0,
            schema_name: String::new(),
            table_name: String::new(),
            job_type: JobType::Other(0),
            query: String::new(),
            cdc_write_source: 0,
            sql_mode: 0,
            reorg_meta: None,
            session_vars: Default::default(),
            start_ts: 0,
            bdr_role: String::new(),
            state: JobState::None,
            trace_info_present: false,
            binlog_info_present: false,
            need_reorg: false,
            involving_schemas: Vec::new(),
            sub_jobs: Vec::new(),
            admin_operator_system: false,
        }
    }
}

impl Job {
    /// 按 Go Job.NormalizeInvolvingSchemaInfo 规范化名称，但不改变顺序或去重。
    pub fn normalize_involving_schema_info(&mut self) {
        self.schema_name = normalize_involving_name(&self.schema_name);
        self.table_name = normalize_involving_name(&self.table_name);
        for (database, table) in &mut self.involving_schemas {
            *database = normalize_involving_name(database);
            *table = normalize_involving_name(table);
        }
    }

    /// 校验 involving schema 信息，并在未显式提供时使用 job 名称回退值。
    pub fn check_involving_schema_info(&self) -> Result<(), Error> {
        let fallback_table = if !self.schema_name.is_empty() && self.table_name.is_empty() {
            "*"
        } else {
            self.table_name.as_str()
        };
        let involving = if self.involving_schemas.is_empty() {
            vec![(self.schema_name.as_str(), fallback_table)]
        } else {
            self.involving_schemas
                .iter()
                .map(|(database, table)| (database.as_str(), table.as_str()))
                .collect()
        };

        for (database, table) in involving {
            if database.is_empty() && table.is_empty() {
                return Err(Error::invalid(
                    "InvolvingSchemaInfo must involve only one type of object among database/table, placement policy, resource group",
                ));
            }
            if database.is_empty() || table.is_empty() {
                return Err(Error::invalid(
                    "DDL job operating on schema or table, must have non-empty name set in InvolvingSchemaInfo",
                ));
            }
            if database == "*" && table != "*" {
                return Err(Error::invalid(
                    "DDL job operating on all databases, must not set table name in InvolvingSchemaInfo",
                ));
            }
        }
        Ok(())
    }

    /// 该类 job 是否可能需要 reorg（数据重组/回填）。
    pub fn may_need_reorg(&self) -> bool {
        matches!(
            self.job_type,
            JobType::AddIndex
                | JobType::AddPrimaryKey
                | JobType::ReorganizePartition
                | JobType::RemovePartitioning
                | JobType::AlterTablePartitioning
        ) || (self.job_type == JobType::ModifyColumn && self.need_reorg)
            || (self.job_type == JobType::MultiSchemaChange
                && self.sub_jobs.iter().any(SubJob::may_need_reorg))
    }

    /// 是否已开始处理（非 None/Queueing）。
    pub fn started(&self) -> bool {
        !matches!(self.state, JobState::None | JobState::Queueing)
    }

    /// 按 Go `model.Job.Encode(true)` 的持久化 wire 格式编码。
    pub fn encode(&self, args: &JobArgs) -> Vec<u8> {
        let mut persistent = model::Job {
            id: self.id,
            tp: self.job_type.code() as model::ActionType,
            schema_id: self.schema_id,
            table_id: self.table_id,
            schema_name: self.schema_name.clone(),
            table_name: self.table_name.clone(),
            state: persistent_job_state(self.state),
            raw_args: encode_job_args(self.version, self.job_type, args),
            start_ts: self.start_ts,
            query: self.query.clone(),
            binlog_info: self.binlog_info_present.then(model::HistoryInfo::default),
            version: if self.version == 2 {
                model::JobVersion::V2
            } else {
                model::JobVersion::V1
            },
            multi_schema_info: (self.job_type == JobType::MultiSchemaChange).then(|| {
                model::MultiSchemaInfo {
                    sub_jobs: self
                        .sub_jobs
                        .iter()
                        .map(|sub| model::SubJob {
                            tp: sub.job_type.unwrap_or(JobType::Other(0)).code()
                                as model::ActionType,
                            raw_args: encode_job_args(
                                self.version,
                                sub.job_type.unwrap_or(JobType::Other(0)),
                                &sub.args,
                            ),
                            state: persistent_job_state(sub.state),
                            need_reorg: sub.need_reorg,
                            ..model::SubJob::default()
                        })
                        .collect(),
                    ..model::MultiSchemaInfo::default()
                }
            }),
            involving_schema_info: self
                .involving_schemas
                .iter()
                .map(|(database, table)| model::InvolvingSchemaInfo {
                    database: database.clone(),
                    table: table.clone(),
                    ..model::InvolvingSchemaInfo::default()
                })
                .collect(),
            admin_operator: if self.admin_operator_system {
                model::AdminCommandOperator::System
            } else {
                model::AdminCommandOperator::NotKnown
            },
            trace_info: self
                .trace_info_present
                .then(model::tracing::TraceInfo::default),
            bdr_role: self.bdr_role.clone(),
            cdc_write_source: self.cdc_write_source,
            sql_mode: self.sql_mode,
            reorg_meta: self.reorg_meta.as_ref().map(|meta| {
                serde_json::from_value(
                    serde_json::to_value(meta.as_ref()).expect("reorg metadata serialization"),
                )
                .expect("reorg metadata snapshot")
            }),
            session_vars: self.session_vars.clone(),
            need_reorg: self.need_reorg,
            ..model::Job::default()
        };
        persistent
            .encode(false)
            .expect("serializing a DDL job wire value cannot fail")
    }
}

fn persistent_job_state(state: JobState) -> model::JobState {
    match state {
        JobState::None => model::JobState::None,
        JobState::Queueing => model::JobState::Queueing,
        JobState::Pausing => model::JobState::Pausing,
        JobState::Paused => model::JobState::Paused,
    }
}

fn encode_job_args(version: i64, job_type: JobType, args: &JobArgs) -> Vec<u8> {
    if let JobArgs::Opaque(raw) = args {
        return raw.clone();
    }

    let value = if version == 2 {
        encode_job_args_v2(args)
    } else {
        encode_job_args_v1(job_type, args)
    };
    serde_json::to_vec(&value).expect("serializing DDL job arguments cannot fail")
}

fn encode_job_args_v2(args: &JobArgs) -> Value {
    match args {
        JobArgs::None | JobArgs::Opaque(_) => Value::Null,
        JobArgs::CreateTable { table } => json!({ "table_info": table_value(table) }),
        JobArgs::BatchCreateTable { tables } => json!({
            "tables": tables
                .iter()
                .map(|table| json!({ "table_info": table_value(table) }))
                .collect::<Vec<_>>()
        }),
        JobArgs::CreateSchema { database } => json!({ "db_info": { "id": database.id } }),
        JobArgs::ResourceGroup { group } => json!({ "rg_info": { "id": group.id } }),
        JobArgs::TablePartition { partition } => json!({ "part_info": partition_value(partition) }),
        JobArgs::TruncateTable {
            old_partition_ids,
            new_table_id,
            new_partition_ids,
        } => json!({
            "new_table_id": new_table_id,
            "new_partition_ids": new_partition_ids,
            "old_partition_ids": old_partition_ids
        }),
        JobArgs::RenameTables { tables } => json!({
            "rename_table_infos": tables.iter().map(rename_table_value).collect::<Vec<_>>()
        }),
        JobArgs::RenameTable { old_schema_id } => json!({ "old_schema_id": old_schema_id }),
        JobArgs::ExchangePartition {
            partition_schema_id,
            partition_table_id,
        } => json!({
            "pt_schema_id": partition_schema_id,
            "pt_table_id": partition_table_id
        }),
    }
}

fn encode_job_args_v1(job_type: JobType, args: &JobArgs) -> Value {
    match args {
        JobArgs::None | JobArgs::Opaque(_) => Value::Null,
        JobArgs::CreateTable { table } => match job_type {
            JobType::CreateTable => json!([table_value(table), false]),
            JobType::CreateView => json!([table_value(table), false, 0]),
            _ => json!([table_value(table)]),
        },
        JobArgs::BatchCreateTable { tables } => {
            json!([tables.iter().map(table_value).collect::<Vec<_>>(), false])
        }
        JobArgs::CreateSchema { database } => json!([{ "id": database.id }]),
        JobArgs::ResourceGroup { group } => json!([{ "id": group.id }, false]),
        JobArgs::TablePartition { partition } => {
            if job_type == JobType::AddTablePartition {
                json!([partition_value(partition)])
            } else {
                json!([[], partition_value(partition)])
            }
        }
        JobArgs::TruncateTable {
            old_partition_ids,
            new_table_id,
            new_partition_ids,
        } => {
            if job_type == JobType::TruncateTable {
                json!([
                    new_table_id,
                    false,
                    new_partition_ids,
                    old_partition_ids.len()
                ])
            } else {
                json!([old_partition_ids, new_partition_ids])
            }
        }
        JobArgs::RenameTables { tables } => json!([
            tables
                .iter()
                .map(|item| item.old_schema_id)
                .collect::<Vec<_>>(),
            tables
                .iter()
                .map(|item| item.new_schema_id)
                .collect::<Vec<_>>(),
            vec![""; tables.len()],
            tables.iter().map(|item| item.table_id).collect::<Vec<_>>(),
            vec![""; tables.len()],
            vec![""; tables.len()]
        ]),
        JobArgs::RenameTable { old_schema_id } => json!([old_schema_id, {}, {}]),
        JobArgs::ExchangePartition {
            partition_schema_id,
            partition_table_id,
        } => json!([0, partition_schema_id, partition_table_id, "", false]),
    }
}

fn table_value(table: &TableInfo) -> Value {
    let mut value = json!({ "id": table.id });
    if let Some(partitions) = &table.partitions {
        value["partition"] = partition_value(partitions);
    }
    value
}

fn partition_value(partition: &PartitionInfo) -> Value {
    json!({
        "definitions": partition
            .definitions
            .iter()
            .map(|definition| json!({ "id": definition.id }))
            .collect::<Vec<_>>()
    })
}

fn rename_table_value(info: &RenameTableInfo) -> Value {
    json!({
        "old_schema_id": info.old_schema_id,
        "new_schema_id": info.new_schema_id,
        "table_id": info.table_id
    })
}

impl SubJob {
    fn may_need_reorg(&self) -> bool {
        matches!(
            self.job_type,
            Some(
                JobType::AddIndex
                    | JobType::AddPrimaryKey
                    | JobType::ReorganizePartition
                    | JobType::RemovePartitioning
                    | JobType::AlterTablePartitioning
            )
        ) || (self.job_type == Some(JobType::ModifyColumn) && self.need_reorg)
    }
}

fn normalize_involving_name(name: &str) -> String {
    if matches!(name, "" | "*") {
        name.to_owned()
    } else {
        name.to_lowercase()
    }
}

/// 一次提交单元：Job + Args，以及是否已预先分配 ID。
#[derive(Clone, Debug, Default)]
pub struct JobSpec {
    pub job: Job,
    pub args: JobArgs,
    /// 为 true 时不再为表/分区等对象分配全局 ID，仅分配 job.id。
    pub id_allocated: bool,
}

/// 会话抽象：事务、全局 ID、SQL 执行等提交路径依赖。
pub trait Session: Send {
    fn begin(&mut self) -> Result<(), Error>;
    fn rollback(&mut self);
    fn commit(&mut self) -> Result<(), Error>;
    fn read_bdr_role_and_start_ts(&mut self) -> Result<(String, u64), Error>;
    fn transaction_start_ts(&self) -> Result<u64, Error>;
    fn set_pessimistic(&mut self);
    fn lock_global_id_key(&mut self, for_update_ts: u64) -> Result<(), Error>;
    fn current_version(&self) -> Result<u64, Error>;
    fn set_snapshot_ts(&mut self, timestamp: u64);
    fn generate_global_ids(&mut self, count: usize) -> Result<Vec<i64>, Error>;
    fn execute(&mut self, sql: &str, label: &str) -> Result<(), Error>;
}

/// 会话池：借出/归还 Session。
pub trait SessionPool: Send + Sync {
    fn get(&self) -> Result<Box<dyn Session>, Error>;
    fn put(&self, session: Box<dyn Session>);
}

/// 系统表管理：查询是否存在 flashback cluster job。
pub trait SystemTableManager: Send + Sync {
    fn has_flashback_cluster_job(&self, min_job_id: i64) -> Result<bool, Error>;
}

/// 提供当前最小 job_id，用于 flashback 冲突检查窗口。
pub trait MinJobIdProvider: Send + Sync {
    fn current_min_job_id(&self) -> i64;
}

/// 服务器状态：是否处于升级中。
pub trait ServerState: Send + Sync {
    fn is_upgrading(&self) -> bool;
}

/// BDR 策略：判断某角色下某类 DDL 是否被拒绝。
pub trait BdrPolicy: Send + Sync {
    fn is_denied(&self, role: &str, job_type: JobType, args: &JobArgs) -> bool;
}

/// DDL owner 通知器：唤醒 owner 处理新入队任务。
pub trait OwnerNotifier: Send + Sync {
    fn notify(&self) -> Result<(), Error>;
}

/// 插入失败时可选的一次性清理回调。
pub type Cleanup = Box<dyn FnOnce() + Send>;
/// 分配 ID 之后、插入之前的钩子；可返回 Cleanup。
pub type BeforeInsert = Arc<dyn Fn(&mut [JobSpec]) -> Option<Cleanup> + Send + Sync>;

/// 批量提交所需的依赖与策略配置。
pub struct SubmitOptions {
    pub session_pool: Arc<dyn SessionPool>,
    pub system_table_manager: Arc<dyn SystemTableManager>,
    pub min_job_id_provider: Arc<dyn MinJobIdProvider>,
    pub server_state: Option<Arc<dyn ServerState>>,
    pub bdr_policy: Arc<dyn BdrPolicy>,
    pub before_insert_with_assigned_ids: Option<BeforeInsert>,
    /// 插入重试上限。
    pub max_retry_count: usize,
    /// 重试退避回调，参数为 attempt 下标。
    pub backoff: Arc<dyn Fn(usize) + Send + Sync>,
}
