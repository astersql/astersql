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

// DDL Job 模型：动作类型、状态机、参数编解码与多 schema 变更子任务。
//
// Job 是 DDL 操作的持久化载体；`raw_args` 为 JSON 协议字段，内存 `args` 为解码缓存。
// SchemaState 描述对象在 online DDL（在线变更）过程中的可见性阶段；
// JobVersion 区分 V1 无类型数组与 V2 有类型结构参数。
// MultiSchemaInfo / SubJob 支持单条语句内多个 schema 变更的调度与回滚边界。

use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

mod raw_json {
    use serde::{
        Deserialize, Deserializer, Serialize, Serializer, de::Error as _, ser::Error as _,
    };

    pub fn serialize<S>(raw: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if raw.is_empty() {
            return serializer.serialize_none();
        }
        let value = serde_json::Value::deserialize(&mut serde_json::Deserializer::from_slice(raw))
            .map_err(S::Error::custom)?;
        value.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        serde_json::to_vec(&value).map_err(D::Error::custom)
    }
}

// ActionType 数值是持久化协议，不能重排；46/48 为已删除动作保留 tombstone，200..256 留给下游 fork。
pub type ActionType = u8;
pub const ACTION_NONE: ActionType = 0;
pub const ACTION_CREATE_SCHEMA: ActionType = 1;
pub const ACTION_DROP_SCHEMA: ActionType = 2;
pub const ACTION_CREATE_TABLE: ActionType = 3;
pub const ACTION_DROP_TABLE: ActionType = 4;
pub const ACTION_ADD_COLUMN: ActionType = 5;
pub const ACTION_DROP_COLUMN: ActionType = 6;
pub const ACTION_ADD_INDEX: ActionType = 7;
pub const ACTION_DROP_INDEX: ActionType = 8;
pub const ACTION_ADD_FOREIGN_KEY: ActionType = 9;
pub const ACTION_DROP_FOREIGN_KEY: ActionType = 10;
pub const ACTION_TRUNCATE_TABLE: ActionType = 11;
pub const ACTION_MODIFY_COLUMN: ActionType = 12;
pub const ACTION_REBASE_AUTO_ID: ActionType = 13;
pub const ACTION_RENAME_TABLE: ActionType = 14;
pub const ACTION_SET_DEFAULT_VALUE: ActionType = 15;
pub const ACTION_SHARD_ROW_ID: ActionType = 16;
pub const ACTION_MODIFY_TABLE_COMMENT: ActionType = 17;
pub const ACTION_RENAME_INDEX: ActionType = 18;
pub const ACTION_ADD_TABLE_PARTITION: ActionType = 19;
pub const ACTION_DROP_TABLE_PARTITION: ActionType = 20;
pub const ACTION_CREATE_VIEW: ActionType = 21;
pub const ACTION_MODIFY_TABLE_CHARSET_AND_COLLATE: ActionType = 22;
pub const ACTION_TRUNCATE_TABLE_PARTITION: ActionType = 23;
pub const ACTION_DROP_VIEW: ActionType = 24;
pub const ACTION_RECOVER_TABLE: ActionType = 25;
pub const ACTION_MODIFY_SCHEMA_CHARSET_AND_COLLATE: ActionType = 26;
pub const ACTION_LOCK_TABLE: ActionType = 27;
pub const ACTION_UNLOCK_TABLE: ActionType = 28;
pub const ACTION_REPAIR_TABLE: ActionType = 29;
pub const ACTION_SET_TIFLASH_REPLICA: ActionType = 30;
pub const ACTION_UPDATE_TIFLASH_REPLICA_STATUS: ActionType = 31;
pub const ACTION_ADD_PRIMARY_KEY: ActionType = 32;
pub const ACTION_DROP_PRIMARY_KEY: ActionType = 33;
pub const ACTION_CREATE_SEQUENCE: ActionType = 34;
pub const ACTION_ALTER_SEQUENCE: ActionType = 35;
pub const ACTION_DROP_SEQUENCE: ActionType = 36;
pub const ACTION_ADD_COLUMNS: ActionType = 37; // 已由 MultiSchemaChange 取代。
pub const ACTION_DROP_COLUMNS: ActionType = 38; // 已由 MultiSchemaChange 取代。
pub const ACTION_MODIFY_TABLE_AUTO_ID_CACHE: ActionType = 39;
pub const ACTION_REBASE_AUTO_RANDOM_BASE: ActionType = 40;
pub const ACTION_ALTER_INDEX_VISIBILITY: ActionType = 41;
pub const ACTION_EXCHANGE_TABLE_PARTITION: ActionType = 42;
pub const ACTION_ADD_CHECK_CONSTRAINT: ActionType = 43;
pub const ACTION_DROP_CHECK_CONSTRAINT: ActionType = 44;
pub const ACTION_ALTER_CHECK_CONSTRAINT: ActionType = 45;
pub const DEPRECATED_ACTION_ALTER_TABLE_ALTER_PARTITION: ActionType = 46;
pub const ACTION_RENAME_TABLES: ActionType = 47;
pub const DEPRECATED_ACTION_DROP_INDEXES: ActionType = 48;
pub const ACTION_ALTER_TABLE_ATTRIBUTES: ActionType = 49;
pub const ACTION_ALTER_TABLE_PARTITION_ATTRIBUTES: ActionType = 50;
pub const ACTION_CREATE_PLACEMENT_POLICY: ActionType = 51;
pub const ACTION_ALTER_PLACEMENT_POLICY: ActionType = 52;
pub const ACTION_DROP_PLACEMENT_POLICY: ActionType = 53;
pub const ACTION_ALTER_TABLE_PARTITION_PLACEMENT: ActionType = 54;
pub const ACTION_MODIFY_SCHEMA_DEFAULT_PLACEMENT: ActionType = 55;
pub const ACTION_ALTER_TABLE_PLACEMENT: ActionType = 56;
pub const ACTION_ALTER_CACHE_TABLE: ActionType = 57;
pub const ACTION_ALTER_TABLE_STATS_OPTIONS: ActionType = 58;
pub const ACTION_ALTER_NO_CACHE_TABLE: ActionType = 59;
pub const ACTION_CREATE_TABLES: ActionType = 60;
pub const ACTION_MULTI_SCHEMA_CHANGE: ActionType = 61;
pub const ACTION_FLASHBACK_CLUSTER: ActionType = 62;
pub const ACTION_RECOVER_SCHEMA: ActionType = 63;
pub const ACTION_REORGANIZE_PARTITION: ActionType = 64;
pub const ACTION_ALTER_TTL_INFO: ActionType = 65;
// 66 由历史协议保留，后续动作必须继续使用既定编号。
pub const ACTION_ALTER_TTL_REMOVE: ActionType = 67;
pub const ACTION_CREATE_RESOURCE_GROUP: ActionType = 68;
pub const ACTION_ALTER_RESOURCE_GROUP: ActionType = 69;
pub const ACTION_DROP_RESOURCE_GROUP: ActionType = 70;
pub const ACTION_ALTER_TABLE_PARTITIONING: ActionType = 71;
pub const ACTION_REMOVE_PARTITIONING: ActionType = 72;
pub const ACTION_ADD_COLUMNAR_INDEX: ActionType = 73;
pub const ACTION_MODIFY_ENGINE_ATTRIBUTE: ActionType = 74;
pub const ACTION_ALTER_TABLE_MODE: ActionType = 75;
pub const ACTION_REFRESH_META: ActionType = 76;
pub const ACTION_MODIFY_SCHEMA_READ_ONLY: ActionType = 77;
pub const ACTION_ALTER_TABLE_AFFINITY: ActionType = 78;
pub const ACTION_ALTER_TABLE_SOFT_DELETE_INFO: ActionType = 79;
pub const ACTION_MODIFY_SCHEMA_SOFT_DELETE_AND_ACTIVE_ACTIVE: ActionType = 80;
pub const ACTION_CREATE_MASKING_POLICY: ActionType = 81;
pub const ACTION_ALTER_MASKING_POLICY: ActionType = 82;
pub const ACTION_DROP_MASKING_POLICY: ActionType = 83;
pub const ACTION_ALTER_TABLE_SET_REGION_SPLIT_POLICY: ActionType = 84;
pub const ACTION_CREATE_MATERIALIZED_VIEW_LOG: ActionType = 85;
pub const ACTION_CREATE_MATERIALIZED_VIEW: ActionType = 86;
pub const ACTION_DROP_MATERIALIZED_VIEW_LOG: ActionType = 87;
pub const ACTION_DROP_MATERIALIZED_VIEW: ActionType = 88;
pub const ACTION_ALTER_MATERIALIZED_VIEW_REFRESH: ActionType = 89;
pub const ACTION_ALTER_MATERIALIZED_VIEW_LOG_PURGE: ActionType = 90;
pub const ACTION_ALTER_MATERIALIZED_VIEW_ATTRIBUTES: ActionType = 91;
pub const ACTION_MVIEW_REFRESH_OUT_OF_PLACE_CUTOVER: ActionType = 92;
pub const ACTION_CREATE_MATERIALIZED_VIEW_SHADOW: ActionType = 93;
pub const ACTION_DROP_MATERIALIZED_VIEW_SHADOW: ActionType = 94;

// 对应 ActionMap/String；未知或 ActionNone 统一显示 none。
pub fn action_type_string(action: ActionType) -> &'static str {
    match action {
        ACTION_CREATE_SCHEMA => "create schema",
        ACTION_DROP_SCHEMA => "drop schema",
        ACTION_CREATE_TABLE => "create table",
        ACTION_CREATE_TABLES => "create tables",
        ACTION_DROP_TABLE => "drop table",
        ACTION_ADD_COLUMN => "add column",
        ACTION_DROP_COLUMN => "drop column",
        ACTION_ADD_INDEX => "add index",
        ACTION_DROP_INDEX => "drop index",
        ACTION_ADD_FOREIGN_KEY => "add foreign key",
        ACTION_DROP_FOREIGN_KEY => "drop foreign key",
        ACTION_TRUNCATE_TABLE => "truncate table",
        ACTION_MODIFY_COLUMN => "modify column",
        ACTION_REBASE_AUTO_ID => "rebase auto_increment ID",
        ACTION_RENAME_TABLE => "rename table",
        ACTION_RENAME_TABLES => "rename tables",
        ACTION_SET_DEFAULT_VALUE => "set default value",
        ACTION_SHARD_ROW_ID => "shard row ID",
        ACTION_MODIFY_TABLE_COMMENT => "modify table comment",
        ACTION_RENAME_INDEX => "rename index",
        ACTION_ADD_TABLE_PARTITION => "add partition",
        ACTION_DROP_TABLE_PARTITION => "drop partition",
        ACTION_CREATE_VIEW => "create view",
        ACTION_MODIFY_TABLE_CHARSET_AND_COLLATE => "modify table charset and collate",
        ACTION_TRUNCATE_TABLE_PARTITION => "truncate partition",
        ACTION_DROP_VIEW => "drop view",
        ACTION_RECOVER_TABLE => "recover table",
        ACTION_MODIFY_SCHEMA_CHARSET_AND_COLLATE => "modify schema charset and collate",
        ACTION_LOCK_TABLE => "lock table",
        ACTION_UNLOCK_TABLE => "unlock table",
        ACTION_REPAIR_TABLE => "repair table",
        ACTION_SET_TIFLASH_REPLICA => "set tiflash replica",
        ACTION_UPDATE_TIFLASH_REPLICA_STATUS => "update tiflash replica status",
        ACTION_ADD_PRIMARY_KEY => "add primary key",
        ACTION_DROP_PRIMARY_KEY => "drop primary key",
        ACTION_CREATE_SEQUENCE => "create sequence",
        ACTION_ALTER_SEQUENCE => "alter sequence",
        ACTION_DROP_SEQUENCE => "drop sequence",
        ACTION_MODIFY_TABLE_AUTO_ID_CACHE => "modify auto id cache",
        ACTION_REBASE_AUTO_RANDOM_BASE => "rebase auto_random ID",
        ACTION_ALTER_INDEX_VISIBILITY => "alter index visibility",
        ACTION_EXCHANGE_TABLE_PARTITION => "exchange partition",
        ACTION_ADD_CHECK_CONSTRAINT => "add check constraint",
        ACTION_DROP_CHECK_CONSTRAINT => "drop check constraint",
        ACTION_ALTER_CHECK_CONSTRAINT => "alter check constraint",
        ACTION_ALTER_TABLE_ATTRIBUTES => "alter table attributes",
        ACTION_ALTER_TABLE_PARTITION_PLACEMENT => "alter table partition placement",
        ACTION_ALTER_TABLE_PARTITION_ATTRIBUTES => "alter table partition attributes",
        ACTION_CREATE_PLACEMENT_POLICY => "create placement policy",
        ACTION_ALTER_PLACEMENT_POLICY => "alter placement policy",
        ACTION_DROP_PLACEMENT_POLICY => "drop placement policy",
        ACTION_MODIFY_SCHEMA_DEFAULT_PLACEMENT => "modify schema default placement",
        ACTION_ALTER_TABLE_PLACEMENT => "alter table placement",
        ACTION_ALTER_CACHE_TABLE => "alter table cache",
        ACTION_ALTER_NO_CACHE_TABLE => "alter table nocache",
        ACTION_ALTER_TABLE_STATS_OPTIONS => "alter table statistics options",
        ACTION_MULTI_SCHEMA_CHANGE => "alter table multi-schema change",
        ACTION_FLASHBACK_CLUSTER => "flashback cluster",
        ACTION_RECOVER_SCHEMA => "flashback schema",
        ACTION_REORGANIZE_PARTITION => "alter table reorganize partition",
        ACTION_ALTER_TTL_INFO => "alter table ttl",
        ACTION_ALTER_TTL_REMOVE => "alter table no_ttl",
        ACTION_CREATE_RESOURCE_GROUP => "create resource group",
        ACTION_ALTER_RESOURCE_GROUP => "alter resource group",
        ACTION_DROP_RESOURCE_GROUP => "drop resource group",
        ACTION_ALTER_TABLE_PARTITIONING => "alter table partition by",
        ACTION_REMOVE_PARTITIONING => "alter table remove partitioning",
        ACTION_ADD_COLUMNAR_INDEX => "add columnar index",
        ACTION_MODIFY_ENGINE_ATTRIBUTE => "modify engine attribute",
        ACTION_ALTER_TABLE_MODE => "alter table mode",
        ACTION_REFRESH_META => "refresh meta",
        ACTION_MODIFY_SCHEMA_READ_ONLY => "modify schema read only",
        ACTION_ALTER_TABLE_AFFINITY => "alter table affinity",
        ACTION_ALTER_TABLE_SOFT_DELETE_INFO => "alter soft delete info",
        ACTION_MODIFY_SCHEMA_SOFT_DELETE_AND_ACTIVE_ACTIVE => {
            "modify schema soft delete and active active"
        }
        ACTION_CREATE_MASKING_POLICY => "create masking policy",
        ACTION_ALTER_MASKING_POLICY => "alter masking policy",
        ACTION_DROP_MASKING_POLICY => "drop masking policy",
        ACTION_ALTER_TABLE_SET_REGION_SPLIT_POLICY => "alter table set region split policy",
        ACTION_CREATE_MATERIALIZED_VIEW_LOG => "create materialized view log",
        ACTION_CREATE_MATERIALIZED_VIEW => "create materialized view",
        ACTION_DROP_MATERIALIZED_VIEW_LOG => "drop materialized view log",
        ACTION_DROP_MATERIALIZED_VIEW => "drop materialized view",
        ACTION_ALTER_MATERIALIZED_VIEW_REFRESH => "alter materialized view refresh",
        ACTION_ALTER_MATERIALIZED_VIEW_LOG_PURGE => "alter materialized view log purge",
        ACTION_ALTER_MATERIALIZED_VIEW_ATTRIBUTES => "alter materialized view attributes",
        ACTION_MVIEW_REFRESH_OUT_OF_PLACE_CUTOVER => {
            "refresh materialized view complete out-of-place cutover"
        }
        ACTION_CREATE_MATERIALIZED_VIEW_SHADOW => "create materialized view shadow table",
        ACTION_DROP_MATERIALIZED_VIEW_SHADOW => "drop materialized view shadow table",
        DEPRECATED_ACTION_ALTER_TABLE_ALTER_PARTITION => "alter partition",
        _ => "none",
    }
}

// ModifyColumnType 保留旧版可能把数值 6 当 mysql.TypeNull 的兼容约束。
pub const MODIFY_TYPE_NONE: u8 = 0;
pub const MODIFY_TYPE_NO_REORG: u8 = 1;
pub const MODIFY_TYPE_NO_REORG_WITH_CHECK: u8 = 2;
pub const MODIFY_TYPE_INDEX_REORG: u8 = 3;
pub const MODIFY_TYPE_REORG: u8 = 4;
pub const MODIFY_TYPE_PRECHECK: u8 = 5;
pub fn modify_type_to_string(tp: u8) -> &'static str {
    match tp {
        MODIFY_TYPE_NONE => "none",
        MODIFY_TYPE_NO_REORG => "modify meta only",
        MODIFY_TYPE_NO_REORG_WITH_CHECK => "modify meta only with range check",
        MODIFY_TYPE_INDEX_REORG => "reorg index only",
        MODIFY_TYPE_REORG => "reorg row and index",
        MODIFY_TYPE_PRECHECK => "prechecking",
        _ => "",
    }
}

// V1 把参数保存为无类型数组；V2 从 8.4.0 起保存单个有类型结构。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default, Serialize_repr, Deserialize_repr)]
#[repr(i64)]
pub enum JobVersion {
    #[default]
    V1 = 1,
    V2 = 2,
}
impl fmt::Display for JobVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::V1 => "v1",
            Self::V2 => "v2",
        })
    }
}
static JOB_VER_IN_USE: AtomicI64 = AtomicI64::new(JobVersion::V1 as i64);
/// 设置进程内默认 Job 协议版本（滚动升级兼容用）。
pub fn set_job_ver_in_use(version: JobVersion) {
    JOB_VER_IN_USE.store(version as i64, Ordering::Release);
}
/// 读取进程内当前使用的 Job 协议版本。
pub fn get_job_ver_in_use() -> JobVersion {
    if JOB_VER_IN_USE.load(Ordering::Acquire) == 2 {
        JobVersion::V2
    } else {
        JobVersion::V1
    }
}

// 锁保护的可变字段对应 Go Job.Mu，同时覆盖 row count 与 reorg warnings 的竞态要求。
#[derive(Default)]
pub struct JobMutable {
    pub row_count: i64,
    pub warnings: HashMap<errors::ErrorID, terror::Error>,
    pub warning_counts: HashMap<errors::ErrorID, i64>,
}

// Job 是 DDL 操作的持久化载体；args 为解码缓存，RawArgs 才是 JSON 协议字段。
pub struct Job {
    pub id: i64,
    pub tp: ActionType,
    pub schema_id: i64,
    pub table_id: i64,
    pub schema_name: String,
    pub table_name: String,
    pub state: JobState,
    pub warning: Option<terror::Error>,
    pub error: Option<terror::Error>,
    pub error_count: i64,
    pub mutable: Mutex<JobMutable>,
    pub need_reorg: bool,
    #[doc(hidden)]
    pub args: Vec<serde_json::Value>,
    pub raw_args: Vec<u8>,
    pub schema_state: SchemaState,
    pub snapshot_ver: u64,
    pub real_start_ts: u64,
    pub start_ts: u64,
    pub dependency_id: i64,
    pub query: String,
    pub binlog_info: Option<HistoryInfo>,
    pub version: JobVersion,
    pub reorg_meta: Option<DDLReorgMeta>,
    pub multi_schema_info: Option<MultiSchemaInfo>,
    pub priority: i32,
    pub seq_num: u64,
    pub charset: String,
    pub collate: String,
    pub involving_schema_info: Vec<InvolvingSchemaInfo>,
    pub admin_operator: AdminCommandOperator,
    pub pause_reason: Option<JobPauseReason>,
    pub resume_reason: Option<JobResumeReason>,
    pub trace_info: Option<tracing::TraceInfo>,
    pub bdr_role: String,
    pub cdc_write_source: u64,
    pub local_mode: bool,
    pub sql_mode: mysql::SQLMode,
    pub session_vars: HashMap<String, String>,
    pub last_schema_version: i64,
    pub ru: f64,
}

// 默认构造：V1、无动作、空参数与未开始状态。
impl Default for Job {
    fn default() -> Self {
        Self {
            id: 0,
            tp: ACTION_NONE,
            schema_id: 0,
            table_id: 0,
            schema_name: String::new(),
            table_name: String::new(),
            state: JobState::None,
            warning: None,
            error: None,
            error_count: 0,
            mutable: Mutex::new(JobMutable::default()),
            need_reorg: false,
            args: Vec::new(),
            raw_args: Vec::new(),
            schema_state: SchemaState::None,
            snapshot_ver: 0,
            real_start_ts: 0,
            start_ts: 0,
            dependency_id: 0,
            query: String::new(),
            binlog_info: None,
            version: JobVersion::V1,
            reorg_meta: None,
            multi_schema_info: None,
            priority: 0,
            seq_num: 0,
            charset: String::new(),
            collate: String::new(),
            involving_schema_info: Vec::new(),
            admin_operator: AdminCommandOperator::NotKnown,
            pause_reason: None,
            resume_reason: None,
            trace_info: None,
            bdr_role: String::new(),
            cdc_write_source: 0,
            local_mode: false,
            sql_mode: 0,
            session_vars: HashMap::new(),
            last_schema_version: 0,
            ru: 0.0,
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
// Job 的 serde 导线形态；字段名对齐持久化 JSON（含 type/err 等 rename）。
struct JobWire {
    id: i64,
    #[serde(rename = "type")]
    tp: ActionType,
    schema_id: i64,
    table_id: i64,
    schema_name: String,
    table_name: String,
    state: JobState,
    warning: Option<terror::Error>,
    #[serde(rename = "err")]
    error: Option<terror::Error>,
    #[serde(rename = "err_count")]
    error_count: i64,
    row_count: i64,
    #[serde(with = "raw_json")]
    raw_args: Vec<u8>,
    schema_state: SchemaState,
    snapshot_ver: u64,
    real_start_ts: u64,
    start_ts: u64,
    dependency_id: i64,
    query: String,
    #[serde(rename = "binlog")]
    binlog_info: Option<HistoryInfo>,
    version: JobVersion,
    reorg_meta: Option<DDLReorgMeta>,
    multi_schema_info: Option<MultiSchemaInfo>,
    priority: i32,
    seq_num: u64,
    charset: String,
    collate: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    involving_schema_info: Vec<InvolvingSchemaInfo>,
    admin_operator: AdminCommandOperator,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pause_reason: Option<JobPauseReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resume_reason: Option<JobResumeReason>,
    trace_info: Option<tracing::TraceInfo>,
    bdr_role: String,
    cdc_write_source: u64,
    local_mode: bool,
    sql_mode: mysql::SQLMode,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    session_vars: HashMap<String, String>,
    last_schema_version: i64,
    #[serde(skip_serializing_if = "is_zero_ru")]
    ru: f64,
}

fn is_zero_ru(value: &f64) -> bool {
    *value == 0.0
}

impl JobWire {
    // 从运行态 Job 快照导线字段；row_count/warnings 经锁取出并写入 reorg_meta。
    fn from_job(job: &Job) -> Self {
        let (warnings, warning_counts) = job.get_warnings();
        let reorg_meta = job.reorg_meta.as_ref().map(|meta| {
            let mut copy = meta.ShallowCopy();
            copy.Warnings = warnings;
            copy.WarningsCount = warning_counts;
            copy
        });
        Self {
            id: job.id,
            tp: job.tp,
            schema_id: job.schema_id,
            table_id: job.table_id,
            schema_name: job.schema_name.clone(),
            table_name: job.table_name.clone(),
            state: job.state,
            warning: job.warning.clone(),
            error: job.error.clone(),
            error_count: job.error_count,
            row_count: job.get_row_count(),
            raw_args: job.raw_args.clone(),
            schema_state: job.schema_state,
            snapshot_ver: job.snapshot_ver,
            real_start_ts: job.real_start_ts,
            start_ts: job.start_ts,
            dependency_id: job.dependency_id,
            query: job.query.clone(),
            binlog_info: job.binlog_info.clone(),
            version: job.version,
            reorg_meta,
            multi_schema_info: job.multi_schema_info.clone(),
            priority: job.priority,
            seq_num: job.seq_num,
            charset: job.charset.clone(),
            collate: job.collate.clone(),
            involving_schema_info: job.involving_schema_info.clone(),
            admin_operator: job.admin_operator,
            pause_reason: job.pause_reason.clone(),
            resume_reason: job.resume_reason.clone(),
            trace_info: job.trace_info.clone(),
            bdr_role: job.bdr_role.clone(),
            cdc_write_source: job.cdc_write_source,
            local_mode: job.local_mode,
            sql_mode: job.sql_mode,
            session_vars: job.session_vars.clone(),
            last_schema_version: job.last_schema_version,
            ru: job.ru,
        }
    }

    // 导线还原为 Job；warnings 从 reorg_meta 迁回 mutable。
    fn into_job(self) -> Job {
        let (warnings, warning_counts) = self
            .reorg_meta
            .as_ref()
            .map(|meta| (meta.Warnings.clone(), meta.WarningsCount.clone()))
            .unwrap_or_default();
        Job {
            id: self.id,
            tp: self.tp,
            schema_id: self.schema_id,
            table_id: self.table_id,
            schema_name: self.schema_name,
            table_name: self.table_name,
            state: self.state,
            warning: self.warning,
            error: self.error,
            error_count: self.error_count,
            mutable: Mutex::new(JobMutable {
                row_count: self.row_count,
                warnings,
                warning_counts,
            }),
            need_reorg: false,
            args: Vec::new(),
            raw_args: self.raw_args,
            schema_state: self.schema_state,
            snapshot_ver: self.snapshot_ver,
            real_start_ts: self.real_start_ts,
            start_ts: self.start_ts,
            dependency_id: self.dependency_id,
            query: self.query,
            binlog_info: self.binlog_info,
            version: self.version,
            reorg_meta: self.reorg_meta,
            multi_schema_info: self.multi_schema_info,
            priority: self.priority,
            seq_num: self.seq_num,
            charset: self.charset,
            collate: self.collate,
            involving_schema_info: self.involving_schema_info,
            admin_operator: self.admin_operator,
            pause_reason: self.pause_reason,
            resume_reason: self.resume_reason,
            trace_info: self.trace_info,
            bdr_role: self.bdr_role,
            cdc_write_source: self.cdc_write_source,
            local_mode: self.local_mode,
            sql_mode: self.sql_mode,
            session_vars: self.session_vars,
            last_schema_version: self.last_schema_version,
            ru: self.ru,
        }
    }
}

impl Job {
    // 完成任务时同步状态，并把最终 DB/Table 元数据写入 binlog 历史对象；这里不实际写 binlog。
    pub fn finish_table_job(
        &mut self,
        state: JobState,
        schema_state: SchemaState,
        version: i64,
        table: Arc<TableInfo>,
    ) {
        self.state = state;
        self.schema_state = schema_state;
        self.binlog_info
            .get_or_insert_with(HistoryInfo::default)
            .add_table_info(version, table);
    }
    /// 完成多表 Job：写入状态并把多表快照挂到 binlog 历史。
    pub fn finish_multiple_table_job(
        &mut self,
        state: JobState,
        schema_state: SchemaState,
        version: i64,
        tables: Vec<Arc<TableInfo>>,
    ) {
        self.state = state;
        self.schema_state = schema_state;
        let history = self.binlog_info.get_or_insert_with(HistoryInfo::default);
        history.schema_version = version;
        history.table_info = tables.last().cloned();
        history.multiple_table_infos = tables;
    }
    /// 完成库级 Job：写入状态并把 DB 快照挂到 binlog 历史。
    pub fn finish_db_job(
        &mut self,
        state: JobState,
        schema_state: SchemaState,
        version: i64,
        db: Arc<DBInfo>,
    ) {
        self.state = state;
        self.schema_state = schema_state;
        self.binlog_info
            .get_or_insert_with(HistoryInfo::default)
            .add_db_info(version, db);
    }
    /// 标记 multi-schema 变更不可再整体回滚。
    pub fn mark_non_revertible(&mut self) {
        if let Some(info) = &mut self.multi_schema_info {
            info.revertible = false;
        }
    }

    // Clone 沿用 Go 的 Encode(true)+Decode 路径，确保只复制 JSON 可见字段；sub-job 的 JobArgs 再显式恢复。
    pub fn clone_job(&mut self) -> Result<Job, serde_json::Error> {
        let bytes = self.encode(true)?;
        let mut cloned = Job::decode(&bytes)?;
        if let (Some(source), Some(target)) =
            (&self.multi_schema_info, &mut cloned.multi_schema_info)
        {
            for (from, to) in source.sub_jobs.iter().zip(&mut target.sub_jobs) {
                to.job_args = from.job_args.clone();
            }
        }
        Ok(cloned)
    }

    /// 在锁保护下设置已处理行数（重组进度）。
    pub fn set_row_count(&self, count: i64) {
        self.mutable.lock().unwrap().row_count = count;
    }
    /// 读取锁保护的已处理行数。
    pub fn get_row_count(&self) -> i64 {
        self.mutable.lock().unwrap().row_count
    }
    /// 设置重组过程中的告警与计数。
    pub fn set_warnings(
        &self,
        warnings: HashMap<errors::ErrorID, terror::Error>,
        counts: HashMap<errors::ErrorID, i64>,
    ) {
        let mut guard = self.mutable.lock().unwrap();
        guard.warnings = warnings;
        guard.warning_counts = counts;
    }
    /// 克隆取出告警映射与计数。
    pub fn get_warnings(
        &self,
    ) -> (
        HashMap<errors::ErrorID, terror::Error>,
        HashMap<errors::ErrorID, i64>,
    ) {
        let guard = self.mutable.lock().unwrap();
        (guard.warnings.clone(), guard.warning_counts.clone())
    }

    // V1 展开历史数组参数；V2 只允许一个有类型参数对象。
    pub fn fill_args<A: JobArgs>(&mut self, args: &A) {
        self.args = if self.version == JobVersion::V1 {
            args.get_args_v1(self)
        } else {
            vec![args.to_json()]
        };
    }
    /// 按版本填充完成态参数到内存 args（编码前）。
    pub fn fill_finished_args<A: FinishedJobArgs>(&mut self, args: &A) {
        self.args = if self.version == JobVersion::V1 {
            args.get_finished_args_v1(self)
        } else {
            vec![args.to_json()]
        };
    }
    /// 编码 Job 为 JSON；可选先把 args 刷入 raw_args（含 sub-job）。
    pub fn encode(&mut self, update_raw_args: bool) -> Result<Vec<u8>, serde_json::Error> {
        if update_raw_args {
            self.raw_args = marshal_args(self.version, &self.args)?;
            if let Some(info) = &mut self.multi_schema_info {
                for sub in &mut info.sub_jobs {
                    // 只刷新正在执行且已经填充私有 args 的 sub-job。
                    if !sub.args.is_empty() {
                        sub.raw_args = marshal_args(self.version, &sub.args)?;
                    }
                }
            }
        }
        serde_json::to_vec(&JobWire::from_job(self))
    }
    /// 从 JSON 字节解码 Job（不含类型化 args 缓存）。
    pub fn decode(bytes: &[u8]) -> Result<Job, serde_json::Error> {
        serde_json::from_slice::<JobWire>(bytes).map(JobWire::into_job)
    }
    // V1 最多按调用方目标数量解码；多余 raw 参数保持未使用，与 Go 的 min(len(raw), len(args)) 一致。
    pub fn decode_args_v1(&mut self) -> Result<&[serde_json::Value], serde_json::Error> {
        debug_assert_eq!(self.version, JobVersion::V1);
        self.args = serde_json::from_slice(&self.raw_args)?;
        Ok(&self.args)
    }
    /// 清空内存中的解码参数缓存。
    pub fn clear_decoded_args(&mut self) {
        self.args.clear();
    }

    // 以下谓词直接表达 Job FSM，避免调用方重复比较持久化状态数值。
    pub fn is_finished(&self) -> bool {
        matches!(
            self.state,
            JobState::Done | JobState::RollbackDone | JobState::Cancelled
        )
    }
    /// 是否已取消。
    pub fn is_cancelled(&self) -> bool {
        self.state == JobState::Cancelled
    }
    /// 回滚是否已完成。
    pub fn is_rollback_done(&self) -> bool {
        self.state == JobState::RollbackDone
    }
    /// 是否正在回滚。
    pub fn is_rollingback(&self) -> bool {
        self.state == JobState::Rollingback
    }
    /// 是否正在取消。
    pub fn is_cancelling(&self) -> bool {
        self.state == JobState::Cancelling
    }
    /// 是否已暂停。
    pub fn is_paused(&self) -> bool {
        self.state == JobState::Paused
    }
    /// 是否由系统（非终端用户）暂停。
    pub fn is_paused_by_system(&self) -> bool {
        self.is_paused() && self.admin_operator == AdminCommandOperator::System
    }
    /// 暂停原因类型是否匹配。
    pub fn has_pause_reason(&self, reason: &str) -> bool {
        self.pause_reason
            .as_ref()
            .is_some_and(|r| r.reason_type == reason)
    }
    /// 记录暂停原因类型与说明。
    pub fn set_pause_reason(&mut self, reason_type: String, message: String) {
        self.pause_reason = Some(JobPauseReason {
            reason_type,
            message,
        });
    }
    /// 清除暂停原因。
    pub fn clear_pause_reason(&mut self) {
        self.pause_reason = None;
    }
    /// 恢复原因类型是否匹配。
    pub fn has_resume_reason(&self, reason: &str) -> bool {
        self.resume_reason
            .as_ref()
            .is_some_and(|r| r.reason_type == reason)
    }
    /// 记录恢复原因类型。
    pub fn set_resume_reason(&mut self, reason_type: String) {
        self.resume_reason = Some(JobResumeReason { reason_type });
    }
    /// 清除恢复原因。
    pub fn clear_resume_reason(&mut self) {
        self.resume_reason = None;
    }
    /// 是否因 TiKV 磁盘满而由系统暂停。
    pub fn is_paused_by_system_for_kv_disk_full(&self) -> bool {
        self.is_paused_by_system() && self.has_pause_reason(JOB_PAUSE_REASON_KV_DISK_FULL)
    }
    /// 是否正处于或已完成因磁盘满触发的系统暂停。
    pub fn is_pausing_or_paused_by_system_for_kv_disk_full(&self) -> bool {
        matches!(self.state, JobState::Pausing | JobState::Paused)
            && self.admin_operator == AdminCommandOperator::System
            && self.has_pause_reason(JOB_PAUSE_REASON_KV_DISK_FULL)
    }
    /// 是否正在进入暂停。
    pub fn is_pausing(&self) -> bool {
        self.state == JobState::Pausing
    }
    // TiFlash 列式索引处于 write-reorg 时仍不支持 pause。
    pub fn is_pausable(&self) -> bool {
        !(self.tp == ACTION_ADD_COLUMNAR_INDEX
            && self.schema_state == SchemaState::WriteReorganization)
            && (self.not_started() || (self.is_running() && self.is_rollbackable()))
    }
    /// 是否允许 admin alter ddl jobs 在线调整参数。
    pub fn is_alterable(&self) -> bool {
        matches!(
            self.tp,
            ACTION_ADD_INDEX | ACTION_MODIFY_COLUMN | ACTION_REORGANIZE_PARTITION
        )
    }
    /// 是否可恢复（已暂停）。
    pub fn is_resumable(&self) -> bool {
        self.is_paused()
    }
    /// 是否已同步到全部 TiDB 节点。
    pub fn is_synced(&self) -> bool {
        self.state == JobState::Synced
    }
    /// 是否执行完成（待同步）。
    pub fn is_done(&self) -> bool {
        self.state == JobState::Done
    }
    /// 是否正在运行。
    pub fn is_running(&self) -> bool {
        self.state == JobState::Running
    }
    /// 是否在队列中等待。
    pub fn is_queueing(&self) -> bool {
        self.state == JobState::Queueing
    }
    /// 是否尚未开始（None 或 Queueing）。
    pub fn not_started(&self) -> bool {
        matches!(self.state, JobState::None | JobState::Queueing)
    }
    /// 是否已开始执行。
    pub fn started(&self) -> bool {
        !self.not_started()
    }
    // RollbackDone 尚未与真正最终的 rollback-synced 分离，故这里与 Go 一样不计入 final。
    pub fn in_final_state(&self) -> bool {
        matches!(
            self.state,
            JobState::Synced | JobState::Cancelled | JobState::Paused
        )
    }
    /// 记录随 Job 持久化的 session 系统变量。
    pub fn add_system_var(&mut self, name: String, value: String) {
        self.session_vars.insert(name, value);
    }
    /// 读取随 Job 保存的系统变量值。
    pub fn get_system_var(&self, name: &str) -> Option<&str> {
        self.session_vars.get(name).map(String::as_str)
    }

    /// 该动作（或 multi-schema 子任务）是否可能需要数据重组（reorg）。
    pub fn may_need_reorg(&self) -> bool {
        match self.tp {
            ACTION_ADD_INDEX
            | ACTION_ADD_PRIMARY_KEY
            | ACTION_CREATE_MATERIALIZED_VIEW
            | ACTION_REORGANIZE_PARTITION
            | ACTION_REMOVE_PARTITIONING
            | ACTION_ALTER_TABLE_PARTITIONING => true,
            ACTION_MODIFY_COLUMN => self.need_reorg,
            ACTION_MULTI_SCHEMA_CHANGE => self.multi_schema_info.as_ref().is_some_and(|info| {
                info.sub_jobs.iter().any(|sub| {
                    matches!(
                        sub.tp,
                        ACTION_ADD_INDEX
                            | ACTION_ADD_PRIMARY_KEY
                            | ACTION_CREATE_MATERIALIZED_VIEW
                            | ACTION_REORGANIZE_PARTITION
                            | ACTION_REMOVE_PARTITIONING
                            | ACTION_ALTER_TABLE_PARTITIONING
                    ) || (sub.tp == ACTION_MODIFY_COLUMN && sub.need_reorg)
                })
            }),
            _ => false,
        }
    }

    // 回滚边界逐分支对应 convertJob2RollbackJob；进入不可逆 schema state 后拒绝取消。
    pub fn is_rollbackable(&self) -> bool {
        match self.tp {
            ACTION_DROP_INDEX | ACTION_DROP_PRIMARY_KEY => !matches!(
                self.schema_state,
                SchemaState::DeleteOnly
                    | SchemaState::DeleteReorganization
                    | SchemaState::WriteOnly
            ),
            ACTION_MODIFY_COLUMN => self.schema_state != SchemaState::Public,
            ACTION_CREATE_MATERIALIZED_VIEW => matches!(
                self.schema_state,
                SchemaState::None | SchemaState::WriteReorganization
            ),
            ACTION_ADD_TABLE_PARTITION => matches!(
                self.schema_state,
                SchemaState::None | SchemaState::ReplicaOnly
            ),
            ACTION_DROP_COLUMN
            | ACTION_DROP_SCHEMA
            | ACTION_DROP_TABLE
            | ACTION_DROP_SEQUENCE
            | ACTION_DROP_MATERIALIZED_VIEW
            | ACTION_DROP_MATERIALIZED_VIEW_LOG
            | ACTION_DROP_MATERIALIZED_VIEW_SHADOW
            | ACTION_DROP_FOREIGN_KEY
            | ACTION_DROP_TABLE_PARTITION => self.schema_state == SchemaState::Public,
            ACTION_TRUNCATE_TABLE_PARTITION => matches!(
                self.schema_state,
                SchemaState::Public | SchemaState::WriteOnly
            ),
            ACTION_REBASE_AUTO_ID
            | ACTION_SHARD_ROW_ID
            | ACTION_MVIEW_REFRESH_OUT_OF_PLACE_CUTOVER
            | ACTION_TRUNCATE_TABLE
            | ACTION_ADD_FOREIGN_KEY
            | ACTION_RENAME_TABLE
            | ACTION_RENAME_TABLES
            | ACTION_MODIFY_TABLE_CHARSET_AND_COLLATE
            | ACTION_MODIFY_SCHEMA_CHARSET_AND_COLLATE
            | ACTION_REPAIR_TABLE
            | ACTION_MODIFY_TABLE_AUTO_ID_CACHE
            | ACTION_MODIFY_SCHEMA_DEFAULT_PLACEMENT
            | ACTION_DROP_CHECK_CONSTRAINT => self.schema_state == SchemaState::None,
            ACTION_MULTI_SCHEMA_CHANGE => self
                .multi_schema_info
                .as_ref()
                .is_some_and(|info| info.revertible),
            ACTION_FLASHBACK_CLUSTER => !matches!(
                self.schema_state,
                SchemaState::WriteReorganization | SchemaState::WriteOnly
            ),
            ACTION_REORGANIZE_PARTITION
            | ACTION_REMOVE_PARTITIONING
            | ACTION_ALTER_TABLE_PARTITIONING => self.schema_state != SchemaState::Public,
            _ => true,
        }
    }

    // 未显式设置时回退到 SchemaName/TableName；schema 级 DDL 用 * 表示涉及其下全部表。
    pub fn get_involving_schema_info(&self) -> Vec<InvolvingSchemaInfo> {
        if !self.involving_schema_info.is_empty() {
            return self.involving_schema_info.clone();
        }
        let table = if !self.schema_name.is_empty() && self.table_name.is_empty() {
            INVOLVING_ALL.to_owned()
        } else {
            self.table_name.clone()
        };
        vec![InvolvingSchemaInfo {
            database: self.schema_name.clone(),
            table,
            ..Default::default()
        }]
    }
    // 调度器按精确字符串构造依赖 key，除 * 与空哨兵外统一转为小写。
    pub fn normalize_involving_schema_info(&mut self) {
        self.schema_name = normalize_involving_name(&self.schema_name);
        self.table_name = normalize_involving_name(&self.table_name);
        for info in &mut self.involving_schema_info {
            info.database = normalize_involving_name(&info.database);
            info.table = normalize_involving_name(&info.table);
            info.policy = normalize_involving_name(&info.policy);
            info.resource_group = normalize_involving_name(&info.resource_group);
        }
    }
    // 每项只能涉及 database/table、placement policy、resource group 三类之一，否则依赖计算可能卡住或乱序。
    pub fn check_involving_schema_info(&self) -> Result<(), String> {
        for info in self.get_involving_schema_info() {
            let object_types = usize::from(info.policy != INVOLVING_NONE)
                + usize::from(info.resource_group != INVOLVING_NONE)
                + usize::from(info.database != INVOLVING_NONE || info.table != INVOLVING_NONE);
            if object_types != 1 {
                return Err("InvolvingSchemaInfo must involve only one type of object among database/table, placement policy, resource group".into());
            }
            if info.policy == INVOLVING_NONE && info.resource_group == INVOLVING_NONE {
                if info.database == INVOLVING_NONE || info.table == INVOLVING_NONE {
                    return Err("DDL job operating on schema or table, must have non-empty name set in InvolvingSchemaInfo".into());
                }
                if info.database == INVOLVING_ALL && info.table != INVOLVING_ALL {
                    return Err("DDL job operating on all databases, must not set table name in InvolvingSchemaInfo".into());
                }
            }
        }
        Ok(())
    }
}

// 对应 Go Job.String：读取锁保护的 RowCount/Warnings，并附加 modify-column reorg 与 multi-schema 状态。
impl fmt::Display for Job {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ID:{}, Type:{}, State:{}, SchemaState:{}, SchemaID:{}, TableID:{}, RowCount:{}, ArgLen:{}, start time: {:?}, Err:{:?}, ErrCount:{}, SnapshotVersion:{}, Version: {}",
            self.id,
            action_type_string(self.tp),
            self.state,
            self.schema_state.String(),
            self.schema_id,
            self.table_id,
            self.get_row_count(),
            self.args.len(),
            ts_convert_to_time(self.start_ts),
            self.error,
            self.error_count,
            self.snapshot_ver,
            self.version,
        )?;
        if let Some(meta) = &self.reorg_meta {
            if self.tp == ACTION_MODIFY_COLUMN {
                write!(
                    f,
                    ", analyze_state:{}, stage:{}",
                    meta.AnalyzeState, meta.Stage as u8
                )?;
            }
            write!(f, ", UniqueWarnings:{}", self.get_warnings().0.len())?;
        }
        if self.tp != ACTION_MULTI_SCHEMA_CHANGE {
            if let Some(info) = &self.multi_schema_info {
                write!(
                    f,
                    ", Multi-Schema Change:true, Revertible:{}",
                    info.revertible
                )?;
            }
        }
        Ok(())
    }
}

// 参数序列化保留 V1 数组、V2 单对象差异；V2 空 args 序列化为 null。
fn marshal_args(
    version: JobVersion,
    args: &[serde_json::Value],
) -> Result<Vec<u8>, serde_json::Error> {
    if version == JobVersion::V1 {
        if args.is_empty() {
            return Ok(b"null".to_vec());
        }
        return serde_json::to_vec(args);
    }
    debug_assert!(args.len() <= 1, "v2 args should have only one element");
    serde_json::to_vec(args.first().unwrap_or(&serde_json::Value::Null))
}
/// 仅测试可见：替换内存 args 以便模拟参数变更。
pub fn update_job_args_for_test(
    job: &mut Job,
    update: impl FnOnce(Vec<serde_json::Value>) -> Vec<serde_json::Value>,
) {
    if cfg!(test) {
        job.args = update(std::mem::take(&mut job.args));
    }
}
// 依赖 key 规范化：保留 * / 空哨兵，其余转小写。
fn normalize_involving_name(name: &str) -> String {
    if matches!(name, INVOLVING_ALL | INVOLVING_NONE) {
        name.to_owned()
    } else {
        name.to_lowercase()
    }
}

// SubJob 是 multi-schema change 中的单个 schema 变更；私有 args 不参与 Clone。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SubJob {
    pub tp: ActionType,
    #[serde(skip)]
    pub job_args: serde_json::Value,
    #[serde(skip)]
    #[doc(hidden)]
    pub args: Vec<serde_json::Value>,
    #[serde(with = "raw_json")]
    pub raw_args: Vec<u8>,
    pub schema_state: SchemaState,
    pub snapshot_ver: u64,
    pub real_start_ts: u64,
    pub revertible: bool,
    pub state: JobState,
    pub row_count: i64,
    pub warning: Option<terror::Error>,
    #[serde(skip)]
    pub need_reorg: bool,
    pub schema_ver: i64,
    pub reorg_tp: ReorgType,
    pub reorg_stage: ReorgStage,
    pub analyze_state: i8,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub involving_schema_info: Vec<InvolvingSchemaInfo>,
}
impl SubJob {
    /// 子任务是否处于正常（非取消/回滚）路径。
    pub fn is_normal(&self) -> bool {
        !matches!(
            self.state,
            JobState::Cancelling
                | JobState::Cancelled
                | JobState::Rollingback
                | JobState::RollbackDone
        )
    }
    pub fn is_finished(&self) -> bool {
        matches!(
            self.state,
            JobState::Done | JobState::RollbackDone | JobState::Cancelled
        )
    }
    // proxy job 继承 parent 的调度/会话字段，但执行状态、参数和 reorg 进度来自 sub-job。
    pub fn to_proxy_job(&self, parent: &Job, sequence: i32) -> Job {
        let mut reorg = parent.reorg_meta.as_ref().map(DDLReorgMeta::ShallowCopy);
        if let Some(meta) = &mut reorg {
            meta.ReorgTp = self.reorg_tp;
            meta.Stage = self.reorg_stage;
            meta.AnalyzeState = self.analyze_state;
        }
        Job {
            id: parent.id,
            tp: self.tp,
            schema_id: parent.schema_id,
            table_id: parent.table_id,
            schema_name: parent.schema_name.clone(),
            table_name: parent.table_name.clone(),
            state: self.state,
            warning: self.warning.clone(),
            error: None,
            error_count: 0,
            mutable: Mutex::new(JobMutable {
                row_count: self.row_count,
                ..Default::default()
            }),
            need_reorg: self.need_reorg,
            args: self.args.clone(),
            raw_args: self.raw_args.clone(),
            schema_state: self.schema_state,
            snapshot_ver: self.snapshot_ver,
            real_start_ts: self.real_start_ts,
            start_ts: parent.start_ts,
            dependency_id: parent.dependency_id,
            query: parent.query.clone(),
            binlog_info: parent.binlog_info.clone(),
            version: parent.version,
            reorg_meta: reorg,
            multi_schema_info: Some(MultiSchemaInfo {
                revertible: self.revertible,
                seq: sequence,
                ..Default::default()
            }),
            priority: parent.priority,
            seq_num: parent.seq_num,
            charset: parent.charset.clone(),
            collate: parent.collate.clone(),
            involving_schema_info: self.involving_schema_info.clone(),
            admin_operator: parent.admin_operator,
            pause_reason: None,
            resume_reason: parent.resume_reason.clone(),
            trace_info: parent.trace_info.clone(),
            bdr_role: String::new(),
            cdc_write_source: 0,
            local_mode: false,
            sql_mode: parent.sql_mode,
            session_vars: parent.session_vars.clone(),
            last_schema_version: 0,
            ru: parent.ru,
        }
    }
    // 执行完 proxy job 后把可变进度写回 sub-job，parent 级字段不回写。
    pub fn from_proxy_job(&mut self, proxy: &Job, schema_version: i64) {
        self.revertible = proxy
            .multi_schema_info
            .as_ref()
            .is_some_and(|i| i.revertible);
        self.schema_state = proxy.schema_state;
        self.snapshot_ver = proxy.snapshot_ver;
        self.real_start_ts = proxy.real_start_ts;
        self.args = proxy.args.clone();
        self.state = proxy.state;
        self.warning = proxy.warning.clone();
        self.row_count = proxy.get_row_count();
        self.schema_ver = schema_version;
        self.involving_schema_info = proxy.involving_schema_info.clone();
        if let Some(meta) = &proxy.reorg_meta {
            self.reorg_tp = meta.ReorgTp;
            self.reorg_stage = meta.Stage;
            self.analyze_state = meta.AnalyzeState;
        }
    }
    /// 将私有 job_args 填入 args 缓存（V1/V2 形态在此简化为同构）。
    pub fn fill_args(&mut self, version: JobVersion) {
        self.args = if version == JobVersion::V1 {
            vec![self.job_args.clone()]
        } else {
            vec![self.job_args.clone()]
        };
    }
    /// 克隆子任务并清空私有 args 缓存。
    pub fn clone_sub_job(&self) -> SubJob {
        let mut cloned = self.clone();
        cloned.args.clear();
        cloned
    }
}

// MultiSchemaInfo 同时保存持久化 sub-jobs 与执行期冲突检测集合。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MultiSchemaInfo {
    pub sub_jobs: Vec<SubJob>,
    pub revertible: bool,
    pub seq: i32,
    #[serde(skip)]
    pub skip_version: bool,
    #[serde(skip)]
    pub add_columns: Vec<ast::CIStr>,
    #[serde(skip)]
    pub drop_columns: Vec<ast::CIStr>,
    #[serde(skip)]
    pub modify_columns: Vec<ast::CIStr>,
    #[serde(skip)]
    pub add_indexes: Vec<ast::CIStr>,
    #[serde(skip)]
    pub drop_indexes: Vec<ast::CIStr>,
    #[serde(skip)]
    pub alter_indexes: Vec<ast::CIStr>,
    #[serde(skip)]
    pub add_foreign_keys: Vec<AddForeignKeyInfo>,
    #[serde(skip)]
    pub relative_columns: Vec<ast::CIStr>,
    #[serde(skip)]
    pub position_columns: Vec<ast::CIStr>,
    #[serde(skip)]
    pub involving_schema_info: Vec<InvolvingSchemaInfo>,
}
/// 新建默认可回滚的 MultiSchemaInfo。
pub fn new_multi_schema_info() -> MultiSchemaInfo {
    MultiSchemaInfo {
        revertible: true,
        ..Default::default()
    }
}
#[derive(Clone, Debug)]
/// 多 schema 变更中待添加外键的冲突检测信息。
pub struct AddForeignKeyInfo {
    pub name: ast::CIStr,
    pub columns: Vec<ast::CIStr>,
}

// JobMeta 是调度或展示所需的轻量 Job 投影。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobMeta {
    pub schema_id: i64,
    pub table_id: i64,
    pub tp: ActionType,
    pub query: String,
    pub priority: i32,
}

// InvolvingSchemaInfo 三类对象只能设置一类；名称在提交前必须规范为小写。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct InvolvingSchemaInfo {
    pub database: String,
    pub table: String,
    pub policy: String,
    pub resource_group: String,
    pub mode: InvolvingSchemaInfoMode,
}
#[derive(Clone, Copy, Debug, Default, Serialize_repr, Deserialize_repr)]
#[repr(i32)]
/// 涉及对象的锁模式：排他或共享。
pub enum InvolvingSchemaInfoMode {
    #[default]
    Exclusive = 0,
    Shared = 1,
}
/// 表示“全部”的哨兵名称。
pub const INVOLVING_ALL: &str = "*";
/// 表示“未涉及”的空哨兵。
pub const INVOLVING_NONE: &str = "";

// JobState 是持久化 FSM；字符串转换对未知输入回退 None。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default, Serialize_repr, Deserialize_repr)]
#[repr(i32)]
pub enum JobState {
    #[default]
    None = 0,
    Running = 1,
    Rollingback = 2,
    RollbackDone = 3,
    Done = 4,
    Cancelled = 5,
    Synced = 6,
    Cancelling = 7,
    Queueing = 8,
    Paused = 9,
    Pausing = 10,
}
impl fmt::Display for JobState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Running => "running",
            Self::Rollingback => "rollingback",
            Self::RollbackDone => "rollback done",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
            Self::Cancelling => "cancelling",
            Self::Synced => "synced",
            Self::Queueing => "queueing",
            Self::Paused => "paused",
            Self::Pausing => "pausing",
            Self::None => "none",
        })
    }
}
/// 将展示字符串解析为 JobState；未知回退 None。
pub fn str_to_job_state(value: &str) -> JobState {
    match value {
        "running" => JobState::Running,
        "rollingback" => JobState::Rollingback,
        "rollback done" => JobState::RollbackDone,
        "done" => JobState::Done,
        "cancelled" => JobState::Cancelled,
        "cancelling" => JobState::Cancelling,
        "synced" => JobState::Synced,
        "queueing" => JobState::Queueing,
        "paused" => JobState::Paused,
        "pausing" => JobState::Pausing,
        _ => JobState::None,
    }
}

// AdminCommandOperator 区分终端用户与 TiDB 系统发出的 Cancel/Pause/Resume。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default, Serialize_repr, Deserialize_repr)]
#[repr(i32)]
pub enum AdminCommandOperator {
    #[default]
    NotKnown,
    EndUser,
    System,
}
impl fmt::Display for AdminCommandOperator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::EndUser => "EndUser",
            Self::System => "System",
            Self::NotKnown => "None",
        })
    }
}
/// 因 TiKV 磁盘满暂停的原因类型常量。
pub const JOB_PAUSE_REASON_KV_DISK_FULL: &str = "tikv_disk_full";
/// 磁盘满解除后恢复的原因类型常量。
pub const JOB_RESUME_REASON_KV_DISK_FULL: &str = "tikv_disk_full";
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
/// Job 暂停原因：类型键 + 人类可读说明。
pub struct JobPauseReason {
    #[serde(rename = "type", alias = "reason_type", alias = "Type")]
    pub reason_type: String,
    #[serde(alias = "Message", skip_serializing_if = "String::is_empty")]
    pub message: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
/// Job 恢复原因类型。
pub struct JobResumeReason {
    #[serde(rename = "type", alias = "reason_type", alias = "Type")]
    pub reason_type: String,
}

// SchemaDiff 是单一 schema version 的增量，避免 infoschema 每次全量 reload。
#[derive(Clone, Default)]
pub struct SchemaDiff {
    pub version: i64,
    pub tp: ActionType,
    pub schema_id: i64,
    pub table_id: i64,
    pub sub_action_types: Vec<ActionType>,
    pub old_table_id: i64,
    pub old_schema_id: i64,
    pub regenerate_schema_map: bool,
    pub read_table_from_meta: bool,
    pub is_refresh_meta: bool,
    pub affected_options: Vec<AffectedOption>,
}
#[derive(Clone, Default)]
/// SchemaDiff 中受影响的附加 schema/table 选项。
pub struct AffectedOption {
    pub schema_id: i64,
    pub table_id: i64,
    pub old_table_id: i64,
    pub old_schema_id: i64,
}

// HistoryInfo 保存用于 binlog 的最终 schema/table 快照；多表操作另存切片。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HistoryInfo {
    pub schema_version: i64,
    pub db_info: Option<Arc<DBInfo>>,
    pub table_info: Option<Arc<TableInfo>>,
    pub finished_ts: u64,
    pub multiple_table_infos: Vec<Arc<TableInfo>>,
}
impl HistoryInfo {
    /// 记录最终库快照与 schema 版本。
    pub fn add_db_info(&mut self, version: i64, db: Arc<DBInfo>) {
        self.schema_version = version;
        self.db_info = Some(db);
    }
    /// 记录最终表快照与 schema 版本。
    pub fn add_table_info(&mut self, version: i64, table: Arc<TableInfo>) {
        self.schema_version = version;
        self.table_info = Some(table);
    }
    /// 记录多表最终快照。
    pub fn set_table_infos(&mut self, version: i64, tables: &[Arc<TableInfo>]) {
        self.schema_version = version;
        self.multiple_table_infos = tables.to_vec();
    }
    /// 清空历史快照字段。
    pub fn clean(&mut self) {
        self.schema_version = 0;
        self.db_info = None;
        self.table_info = None;
        self.multiple_table_infos.clear();
    }
}

// JobW 同时携带解析后的 Job 和数据库中的原始二进制表示，不会自行持久化。
pub struct JobW {
    pub job: Job,
    pub bytes: Vec<u8>,
}
/// 构造同时持有解析 Job 与原始字节的包装。
pub fn new_job_w(job: Job, bytes: Vec<u8>) -> JobW {
    JobW { job, bytes }
}

// 包初始化默认 V1 以兼容滚动升级；NextGen 无旧节点兼容负担，直接使用 V2。
pub fn init_job_version() {
    set_job_ver_in_use(if kerneltype::is_next_gen() {
        JobVersion::V2
    } else {
        JobVersion::V1
    });
}
