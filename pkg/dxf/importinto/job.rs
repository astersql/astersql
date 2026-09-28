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

// IMPORT INTO 作业提交与运行时进度展示。
//
// 负责将逻辑计划提交为 DXF 任务（可与 SQL job 同事务创建），
// 以及聚合子任务摘要生成 `RuntimeInfo`（进度百分比、ETA、速度等）。
// DXF（Distributed eXecution Framework）为分布式执行框架。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use astersql_config_kerneltype as kerneltype;
use astersql_domain_infosync as infosync;
use astersql_dxf_framework_handle as dxfhandle;
use astersql_dxf_framework_proto as dxfproto;
use astersql_dxf_framework_storage as storage;
use astersql_dxf_framework_taskexecutor_execute as execute;
use astersql_dxf_importinto_taskkey::taskkey;
use astersql_errors as errors;
use astersql_executor_importer as importer;
use astersql_parser_mysql as mysql;
use astersql_types as types;

use crate::planner::LogicalPlan;
use crate::proto::{ServerInfo, TaskMeta};

#[derive(Clone, Debug, Eq, PartialEq)]
/// 一次成功提交后返回的作业/任务标识。
pub struct SubmittedTask {
    /// SQL 导入作业 ID。
    pub JobID: i64,
    /// DXF 全局任务 ID。
    pub TaskID: i64,
    /// 稳定任务键（含类型与 job id，NextGen 可含 keyspace）。
    pub TaskKey: String,
}

/// IMPORT INTO 所需的事务边界：实现须在同一事务内创建 SQL job 与 DXF task
/// （或完成文档所述的 NextGen keyspace 交接）后再返回成功。
/// Transactional boundary required by IMPORT INTO: implementations create the
/// SQL job and DXF task in one transaction (or perform the documented
/// next-generation keyspace handoff) before returning success.
pub trait TaskSubmissionService: Send + Sync {
    /// 创建导入 job 与对应 DXF task，返回已提交标识。
    fn CreateJobAndTask(
        &self,
        logical_plan: &LogicalPlan,
        task_key: &str,
        thread_count: i32,
        max_node_count: i32,
    ) -> Result<SubmittedTask, errors::SharedError>;
}

/// Classic 模式在创建 SQL job 的同一事务中切换表模式。
pub trait ClassicTableModeChanger: Send + Sync {
    fn AlterTableModeForImport(
        &self,
        session: &storage::sessionctx::Context,
        database_id: i64,
        table_id: i64,
    ) -> Result<(), errors::SharedError>;
}

/// 生产接线：DDL 操作由与 SQL job 事务相同的 session backend 执行。
pub struct StorageSessionTableModeChanger;

impl ClassicTableModeChanger for StorageSessionTableModeChanger {
    fn AlterTableModeForImport(
        &self,
        session: &storage::sessionctx::Context,
        database_id: i64,
        table_id: i64,
    ) -> Result<(), errors::SharedError> {
        session
            .AlterTableModeForImport(database_id, table_id)
            .map_err(|error| errors::New(error.to_string()))
    }
}

/// 使用 DXF storage 的事务和 SQL 会话执行完整提交顺序。
/// `running_on_user_keyspace` 由创建服务的真实存储运行时提供，避免从测试用
/// `storage::config` 的默认空 keyspace 推断用户/系统 KS。
pub struct StorageTaskSubmissionService {
    pub running_on_user_keyspace: bool,
    pub table_mode_changer: Arc<dyn ClassicTableModeChanger>,
    local_manager: storage::TaskManager,
    dxf_manager: Option<storage::TaskManager>,
    target_scope_override: Option<String>,
    classic_kernel_override: Option<bool>,
}

impl StorageTaskSubmissionService {
    /// 从已初始化的本地/DXF 服务存储构造生产提交服务。
    pub fn NewWithStorageBackend(
        running_on_user_keyspace: bool,
    ) -> Result<Self, errors::SharedError> {
        Self::New(
            running_on_user_keyspace,
            Arc::new(StorageSessionTableModeChanger),
        )
    }

    pub fn New(
        running_on_user_keyspace: bool,
        table_mode_changer: Arc<dyn ClassicTableModeChanger>,
    ) -> Result<Self, errors::SharedError> {
        let local_manager =
            storage::GetTaskManager().map_err(|error| errors::New(error.to_string()))?;
        Ok(Self {
            running_on_user_keyspace,
            table_mode_changer,
            local_manager,
            dxf_manager: None,
            target_scope_override: None,
            classic_kernel_override: None,
        })
    }

    pub fn WithManagers(
        running_on_user_keyspace: bool,
        table_mode_changer: Arc<dyn ClassicTableModeChanger>,
        local_manager: storage::TaskManager,
        dxf_manager: storage::TaskManager,
        target_scope: String,
        classic_kernel: bool,
    ) -> Self {
        Self {
            running_on_user_keyspace,
            table_mode_changer,
            local_manager,
            dxf_manager: Some(dxf_manager),
            target_scope_override: Some(target_scope),
            classic_kernel_override: Some(classic_kernel),
        }
    }

    fn submit_dxf_task(
        manager: &storage::TaskManager,
        session: storage::sessionctx::Context,
        plan: &LogicalPlan,
        job_id: i64,
        thread_count: i32,
        max_node_count: i32,
        target_scope_override: Option<&str>,
    ) -> Result<i64, storage::Error> {
        let meta = plan
            .ToTaskMeta()
            .map_err(|error| storage::Error::new(error.to_string()))?;
        let target_scope = match target_scope_override {
            Some(scope) => scope.to_owned(),
            None => dxfhandle::GetTargetScope()
                .map_err(|error| storage::Error::new(error.to_string()))?,
        };
        let extra = plan.GetTaskExtraParams();
        manager.CreateTaskWithSession(
            (),
            session,
            TaskKey(job_id),
            storage::proto::ImportInto,
            plan.Plan.Keyspace.clone(),
            thread_count,
            target_scope,
            max_node_count,
            storage::proto::ExtraParams {
                ManualRecovery: extra.ManualRecovery,
                PauseOnKVDiskFull: extra.PauseOnKVDiskFull,
                MaxRuntimeSlots: extra.MaxRuntimeSlots,
                TargetSteps: extra.TargetSteps,
                PrepareMode: extra.PrepareMode,
            },
            meta,
        )
    }

    fn create_import_job(
        session: &storage::sessionctx::Context,
        plan: &importer::Plan,
    ) -> Result<i64, storage::Error> {
        let table = plan
            .TableInfo
            .as_ref()
            .ok_or_else(|| storage::Error::new("IMPORT INTO plan is missing table metadata"))?;
        let parameters = plan.Parameters.as_ref();
        let encoded_parameters = if let Some(parameters) = parameters {
            let mut fields = serde_json::Map::new();
            if !parameters.ColumnsAndVars.is_empty() {
                fields.insert(
                    "columns-and-vars".into(),
                    parameters.ColumnsAndVars.clone().into(),
                );
            }
            if !parameters.SetClause.is_empty() {
                fields.insert("set-clause".into(), parameters.SetClause.clone().into());
            }
            fields.insert(
                "file-location".into(),
                parameters.FileLocation.clone().into(),
            );
            fields.insert("format".into(), parameters.Format.clone().into());
            if !parameters.Options.is_empty() {
                fields.insert(
                    "options".into(),
                    serde_json::to_value(&parameters.Options)
                        .map_err(|error| storage::Error::new(error.to_string()))?,
                );
            }
            serde_json::to_vec(&fields)
        } else {
            serde_json::to_vec(&serde_json::Value::Null)
        }
        .map_err(|error| storage::Error::new(error.to_string()))?;
        storage::sqlexec::ExecSQL(
            (),
            session.GetSQLExecutor(),
            "INSERT INTO mysql.tidb_import_jobs\n            (table_schema, table_name, table_id, group_key, created_by, parameters, source_file_size, status, step)\n            VALUES (%?, %?, %?, %?, %?, %?, %?, %?, %?);",
            vec![
                plan.DBName.clone().into(),
                table.Name.L.clone().into(),
                table.ID.into(),
                plan.GroupKey.clone().into(),
                plan.User.clone().into(),
                encoded_parameters.into(),
                plan.TotalFileSize.into(),
                importer::jobStatusPending.into(),
                importer::jobStepNone.into(),
            ],
        )?;
        let rows = storage::sqlexec::ExecSQL(
            (),
            session.GetSQLExecutor(),
            "SELECT LAST_INSERT_ID();",
            vec![],
        )?;
        if rows.len() != 1 {
            return Err(storage::Error::new(format!(
                "unexpected result length: {}",
                rows.len()
            )));
        }
        Ok(rows[0].GetInt64(0))
    }
}

impl TaskSubmissionService for StorageTaskSubmissionService {
    fn CreateJobAndTask(
        &self,
        logical_plan: &LogicalPlan,
        _task_key: &str,
        thread_count: i32,
        max_node_count: i32,
    ) -> Result<SubmittedTask, errors::SharedError> {
        let local_manager = self.local_manager.clone();
        let mut job_id = 0;
        let mut task_id = 0;
        let mut submitted_plan = LogicalPlan {
            JobID: logical_plan.JobID,
            Plan: logical_plan.Plan.clone(),
            Stmt: logical_plan.Stmt.clone(),
            EligibleInstances: logical_plan.EligibleInstances.clone(),
            ChunkMap: logical_plan.ChunkMap.clone(),
            PrepareMode: logical_plan.PrepareMode,
            PreparedChunkMapExternalPath: logical_plan.PreparedChunkMapExternalPath.clone(),
            ..Default::default()
        };
        local_manager
            .WithNewTxn((), |session| {
                job_id = Self::create_import_job(&session, &submitted_plan.Plan)?;
                if self
                    .classic_kernel_override
                    .unwrap_or_else(kerneltype::IsClassic)
                {
                    self.table_mode_changer
                        .AlterTableModeForImport(
                            &session,
                            submitted_plan.Plan.DBID,
                            submitted_plan.Plan.TableInfo.as_ref().unwrap().ID,
                        )
                        .map_err(|error| storage::Error::new(error.to_string()))?;
                }
                if !self.running_on_user_keyspace {
                    submitted_plan.JobID = job_id;
                    task_id = Self::submit_dxf_task(
                        &local_manager,
                        session,
                        &submitted_plan,
                        job_id,
                        thread_count,
                        max_node_count,
                        self.target_scope_override.as_deref(),
                    )?;
                }
                Ok(())
            })
            .map_err(|error| errors::New(error.to_string()))?;
        let dxf_manager = if self.running_on_user_keyspace {
            let manager = self
                .dxf_manager
                .clone()
                .map(Ok)
                .unwrap_or_else(storage::GetDXFSvcTaskMgr)
                .map_err(|error| errors::New(error.to_string()))?;
            manager
                .WithNewTxn((), |session| {
                    submitted_plan.JobID = job_id;
                    task_id = Self::submit_dxf_task(
                        &manager,
                        session,
                        &submitted_plan,
                        job_id,
                        thread_count,
                        max_node_count,
                        self.target_scope_override.as_deref(),
                    )?;
                    Ok(())
                })
                .map_err(|error| errors::New(error.to_string()))?;
            manager
        } else {
            local_manager
        };
        dxfhandle::NotifyTaskChange();
        let task = dxf_manager
            .GetTaskBaseByID((), task_id)
            .map_err(|error| errors::New(error.to_string()))?;
        Ok(SubmittedTask {
            JobID: job_id,
            TaskID: task.ID,
            TaskKey: task.Key,
        })
    }
}

/// 提交单机/指定实例的 IMPORT INTO 任务（携带 eligible_instances 与 chunk_map）。
pub fn SubmitStandaloneTask(
    service: &dyn TaskSubmissionService,
    plan: &mut importer::Plan,
    statement: &str,
    chunk_map: HashMap<i32, Vec<importer::Chunk>>,
) -> Result<SubmittedTask, errors::SharedError> {
    let server = infosync::GetServerInfo().map_err(|error| errors::New(error.to_string()))?;
    let instance = ServerInfo {
        id: server.ID,
        ip: server.IP,
        listening_port: server.Port,
    };
    doSubmitTask(service, plan, statement, vec![instance], chunk_map)
}

/// 提交常规 IMPORT INTO 任务（无预分配实例与 chunk 映射）。
pub fn SubmitTask(
    service: &dyn TaskSubmissionService,
    plan: &mut importer::Plan,
    statement: &str,
) -> Result<SubmittedTask, errors::SharedError> {
    doSubmitTask(service, plan, statement, Vec::new(), HashMap::new())
}

/// NextGen 且全局排序时启用异步 prepare（先轻量登记再异步准备）。
pub fn ShouldUseAsyncPrepare(plan: &importer::Plan) -> bool {
    kerneltype::IsNextGen() && plan.IsGlobalSort()
}

/// 组装 LogicalPlan 并调用提交服务；异步 prepare 时强制并发与节点数为 1。
fn doSubmitTask(
    service: &dyn TaskSubmissionService,
    plan: &mut importer::Plan,
    statement: &str,
    eligible_instances: Vec<ServerInfo>,
    chunk_map: HashMap<i32, Vec<importer::Chunk>>,
) -> Result<SubmittedTask, errors::SharedError> {
    // 缺少表元数据或非法 table ID 时直接失败。
    let table = plan
        .TableInfo
        .as_ref()
        .ok_or_else(|| errors::New("IMPORT INTO plan is missing table metadata"))?;
    if table.ID == 0 {
        return Err(errors::New("IMPORT INTO plan has an invalid table ID"));
    }
    let async_prepare = ShouldUseAsyncPrepare(plan);
    let mut logical_plan = LogicalPlan {
        Plan: plan.clone(),
        Stmt: statement.to_owned(),
        EligibleInstances: eligible_instances,
        ChunkMap: chunk_map,
        PrepareMode: if async_prepare {
            dxfproto::PrepareModeRequired
        } else {
            dxfproto::PrepareModeDisabled
        },
        ..Default::default()
    };
    let thread_count = if async_prepare {
        plan.ThreadCnt = 1;
        plan.MaxNodeCnt = 1;
        1
    } else {
        i32::try_from(plan.ThreadCnt)
            .map_err(|_| errors::New("IMPORT INTO thread count exceeds i32"))?
    };
    let max_node_count = if async_prepare { 1 } else { plan.MaxNodeCnt };

    // 提交实现会在事务内分配 job ID；稳定 key 前缀单独传入，
    // 以便在持久化任务元数据前替换为真实 ID。
    // A submission implementation assigns the job ID transactionally. The
    // stable key prefix is passed separately so it can substitute the assigned
    // ID before persisting task metadata.
    logical_plan.JobID = 0;
    service.CreateJobAndTask(&logical_plan, &TaskKey(0), thread_count, max_node_count)
}

#[derive(Clone, Debug)]
/// 单个子任务的运行时摘要（已处理量、行数、速度、更新时间）。
pub struct SubtaskRuntimeSummary {
    /// 当前步骤已处理量（字节或冲突数，视步骤而定）。
    pub Processed: i64,
    /// 已导入/处理行数。
    pub RowCount: i64,
    /// 近期处理速度（每秒）。
    pub Speed: i64,
    /// 摘要最后更新时间。
    pub UpdateTime: SystemTime,
}

#[derive(Clone, Debug)]
/// 从运行时提供者拉取的任务快照（状态、步骤、元数据、子任务列表）。
pub struct TaskRuntimeSnapshot {
    /// 任务状态机当前状态。
    pub State: dxfproto::TaskState,
    /// 当前导入步骤（encode/merge/ingest/冲突处理等）。
    pub Step: dxfproto::Step,
    /// 序列化的 TaskMeta。
    pub Meta: Vec<u8>,
    /// 失败时的错误信息。
    pub ErrorMessage: Option<String>,
    /// 各子任务摘要。
    pub Subtasks: Vec<SubtaskRuntimeSummary>,
}

/// 运行时信息提供者：按 task key 查询任务快照与 job 最后更新时间。
pub trait RuntimeInfoProvider: Send + Sync {
    /// 获取任务运行时快照。
    fn GetTaskRuntime(&self, task_key: &str) -> Result<TaskRuntimeSnapshot, errors::SharedError>;
    /// 获取作业最后更新时间（可能尚无记录）。
    fn GetJobLastUpdateTime(
        &self,
        task_key: &str,
    ) -> Result<Option<SystemTime>, errors::SharedError>;
}

/// 从 DXF 服务任务表与当前 step 的持久化摘要读取运行时快照。
pub struct StorageRuntimeInfoProvider {
    manager: storage::TaskManager,
}

impl StorageRuntimeInfoProvider {
    pub fn New() -> Result<Self, errors::SharedError> {
        Ok(Self {
            manager: storage::GetDXFSvcTaskMgr().map_err(|error| errors::New(error.to_string()))?,
        })
    }

    pub fn WithManager(manager: storage::TaskManager) -> Self {
        Self { manager }
    }
}

#[derive(serde::Deserialize)]
struct WireProgress {
    #[serde(default, rename = "row_count")]
    row_count: i64,
    #[serde(default, rename = "bytes")]
    bytes: i64,
    #[serde(default, rename = "update_time")]
    update_time: Option<String>,
}

#[derive(serde::Deserialize)]
struct WireSubtaskSummary {
    #[serde(default, rename = "row_count")]
    row_count: i64,
    #[serde(default, rename = "bytes")]
    bytes: i64,
    #[serde(default)]
    progresses: Vec<WireProgress>,
}

impl RuntimeInfoProvider for StorageRuntimeInfoProvider {
    fn GetTaskRuntime(&self, task_key: &str) -> Result<TaskRuntimeSnapshot, errors::SharedError> {
        let task = self
            .manager
            .GetTaskByKeyWithHistory((), task_key.to_owned())
            .map_err(|error| errors::New(error.to_string()))?;
        // Go decodes task meta before inspecting task.Error or reading summaries.
        TaskMeta::Unmarshal(&task.Meta)?;
        let mut subtasks = Vec::new();
        if task.Error.is_none() {
            let rows = self.manager.ExecuteSQLWithNewSession(
                (),
                "select summary from mysql.tidb_background_subtask where task_key = %? and step = %?",
                vec![task.ID.into(), task.Step.into()],
            ).map_err(|error| errors::New(error.to_string()))?;
            let now = SystemTime::now();
            for row in rows {
                let wire: WireSubtaskSummary = serde_json::from_slice(&row.GetBytes(0))
                    .map_err(|error| errors::New(error.to_string()))?;
                let mut summary = execute::SubtaskSummary::default();
                summary.Progresses = wire
                    .progresses
                    .into_iter()
                    .map(|point| {
                        let time = point
                            .update_time
                            .as_deref()
                            .map(chrono::DateTime::parse_from_rfc3339)
                            .transpose()
                            .map_err(|error| errors::New(error.to_string()))?
                            .map(|time| time.with_timezone(&chrono::Utc).into())
                            .unwrap_or(SystemTime::UNIX_EPOCH);
                        Ok(execute::Progress {
                            RowCnt: point.row_count,
                            Processed: point.bytes,
                            UpdateTime: time,
                        })
                    })
                    .collect::<Result<Vec<_>, errors::SharedError>>()?;
                subtasks.push(SubtaskRuntimeSummary {
                    Processed: wire.bytes,
                    RowCount: wire.row_count,
                    Speed: summary.GetSpeedInTimeRange(now, execute::SubtaskSpeedUpdateInterval),
                    UpdateTime: summary.UpdateTime(),
                });
            }
        }
        Ok(TaskRuntimeSnapshot {
            State: task.State,
            Step: task.Step,
            Meta: task.Meta,
            ErrorMessage: task.Error,
            Subtasks: subtasks,
        })
    }

    fn GetJobLastUpdateTime(
        &self,
        task_key: &str,
    ) -> Result<Option<SystemTime>, errors::SharedError> {
        let task = self
            .manager
            .GetTaskBaseByKeyWithHistory((), task_key.to_owned())
            .map_err(|error| errors::New(error.to_string()))?;
        let mut last_update = None;
        self.manager.WithNewTxn((), |session| {
            let rows = storage::sqlexec::ExecSQL(
                (), session.GetSQLExecutor(),
                "select FROM_UNIXTIME(max(state_update_time)) from\n                    (select state_update_time from mysql.tidb_background_subtask where task_key = %?\n                        union\n                        select state_update_time from mysql.tidb_background_subtask_history where task_key = %?\n                    ) t",
                vec![task.ID.into(), task.ID.into()],
            )?;
            let row = rows.first().ok_or_else(|| storage::Error::new("last update query returned no row"))?;
            if !row.IsNull(0) {
                last_update = Some(row.GetTime(0).0);
            }
            Ok(())
        }).map_err(|error| errors::New(error.to_string()))?;
        Ok(last_update)
    }
}

#[derive(Clone, Debug)]
/// 面向 SHOW IMPORT JOB 等展示的运行时信息聚合结果。
pub struct RuntimeInfo {
    /// 任务状态。
    pub Status: dxfproto::TaskState,
    /// 已导入行数（步骤相关：部分步骤清零或取最终汇总）。
    pub ImportRows: i64,
    /// 错误消息；非空时提前返回，不再聚合进度。
    pub ErrorMsg: String,
    /// 当前步骤。
    pub Step: dxfproto::Step,
    /// 子任务中最新的更新时间。
    pub UpdateTime: Option<types::time::Time>,
    /// 聚合速度。
    pub Speed: i64,
    /// 聚合已处理量。
    pub Processed: i64,
    /// 当前步骤总量（来自 TaskMeta Summary）。
    pub Total: i64,
}

impl Default for RuntimeInfo {
    /// 默认待处理、初始化步骤、空进度。
    fn default() -> Self {
        Self {
            Status: dxfproto::TaskStatePending,
            ImportRows: 0,
            ErrorMsg: String::new(),
            Step: dxfproto::StepInit,
            UpdateTime: None,
            Speed: 0,
            Processed: 0,
            Total: 0,
        }
    }
}

/// 进度/ETA 不可用时的展示占位。
const NOT_AVAILABLE: &str = "N/A";

impl RuntimeInfo {
    /// 是否处于冲突收集或冲突解决步骤（用量词 conflicts 而非字节）。
    fn isConflictStep(&self) -> bool {
        matches!(
            self.Step,
            dxfproto::ImportStepCollectConflicts | dxfproto::ImportStepConflictResolution
        )
    }

    /// 进度百分比字符串；post-process/init 或总量无效时返回 N/A 或 0。
    pub fn Percent(&self) -> String {
        if matches!(
            self.Step,
            dxfproto::ImportStepPostProcess | dxfproto::StepInit
        ) {
            return NOT_AVAILABLE.to_owned();
        }
        if self.Total <= 0 {
            return "0".to_owned();
        }
        (((self.Processed as f64 / self.Total as f64).min(1.0) * 100.0) as i64).to_string()
    }

    /// 预计剩余时间；速度或总量无效时返回 N/A。
    pub fn ETA(&self) -> String {
        if self.Speed <= 0 || self.Total <= 0 {
            return NOT_AVAILABLE.to_owned();
        }
        FormatSecondAsTime(((self.Total - self.Processed) / self.Speed).max(0))
    }

    /// 当前步骤总量的人类可读形式。
    pub fn TotalSize(&self) -> String {
        if self.isConflictStep() {
            format!("{} conflicts", self.Total)
        } else {
            format_bytes(self.Total)
        }
    }

    /// 当前步骤已处理量的人类可读形式。
    pub fn ProcessedSize(&self) -> String {
        if self.isConflictStep() {
            format!("{} conflicts", self.Processed)
        } else {
            format_bytes(self.Processed)
        }
    }

    /// 速度的人类可读形式。
    pub fn SpeedStr(&self) -> String {
        if self.isConflictStep() {
            format!("{} conflicts/s", self.Speed)
        } else {
            format!("{}/s", format_bytes(self.Speed))
        }
    }
}

/// 将字节数格式化为 B/KiB/MiB/... 字符串。
fn format_bytes(value: i64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB", "ZiB", "YiB"];
    let mut size = value as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    // docker/go-units BytesSize uses `%.4g%s`: four significant digits,
    // without a space, and only scales values >= 1024 (including negative values).
    let exponent = if size == 0.0 {
        0
    } else {
        size.abs().log10().floor() as i32
    };
    let digits = (3 - exponent).max(0) as usize;
    let formatted = if exponent >= 4 {
        let scientific = format!("{size:.3e}");
        let (mantissa, power) = scientific.split_once('e').expect("scientific notation");
        let mantissa = mantissa.trim_end_matches('0').trim_end_matches('.');
        let power: i32 = power.parse().expect("scientific exponent");
        format!("{mantissa}e{power:+03}")
    } else {
        let fixed = format!("{size:.digits$}");
        if fixed.contains('.') {
            fixed.trim_end_matches('0').trim_end_matches('.').to_owned()
        } else {
            fixed
        }
    };
    format!("{formatted}{}", UNITS[unit])
}

/// 将秒数格式化为 `HH:MM:SS`，超过一天则带 `N d` 前缀。
pub fn FormatSecondAsTime(seconds: i64) -> String {
    // Go converts seconds to a signed nanosecond time.Duration, including its
    // overflow and truncation behavior, before formatting each component.
    let duration = seconds.wrapping_mul(1_000_000_000) as f64;
    let total_hours = duration / 3_600_000_000_000.0;
    let hours = total_hours as i64 % 24;
    let minutes = (duration / 60_000_000_000.0) as i64 % 60;
    let seconds = (duration / 1_000_000_000.0) as i64 % 60;
    if total_hours >= 24.0 {
        let days = (total_hours / 24.0) as i64;
        format!("{days} d {hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    }
}

/// 按 job ID 聚合运行时信息：合并子任务摘要并按步骤选取 Total/ImportRows。
pub fn GetRuntimeInfoForJob(
    provider: &dyn RuntimeInfoProvider,
    location: chrono_tz::Tz,
    job_id: i64,
) -> Result<RuntimeInfo, errors::SharedError> {
    let snapshot = provider.GetTaskRuntime(&TaskKey(job_id))?;
    let task_meta = TaskMeta::Unmarshal(&snapshot.Meta)?;
    let has_error = snapshot.ErrorMessage.is_some();
    let mut info = RuntimeInfo {
        Status: snapshot.State,
        Step: snapshot.Step,
        ErrorMsg: snapshot.ErrorMessage.unwrap_or_default(),
        ..Default::default()
    };
    // 已有错误则不再填充进度字段。
    if has_error {
        return Ok(info);
    }
    let mut latest_time = None;
    for summary in snapshot.Subtasks {
        info.Processed = info.Processed.wrapping_add(summary.Processed);
        info.ImportRows = info.ImportRows.wrapping_add(summary.RowCount);
        info.Speed = info.Speed.wrapping_add(summary.Speed);
        if latest_time
            .map(|current| summary.UpdateTime > current)
            .unwrap_or(true)
        {
            latest_time = Some(summary.UpdateTime);
        }
    }
    // post-process 使用任务最终 ImportedRows；非写入类步骤清零 ImportRows。
    if info.Step == dxfproto::ImportStepPostProcess {
        info.ImportRows = task_meta.Summary.ImportedRows;
    } else if !matches!(
        info.Step,
        dxfproto::ImportStepWriteAndIngest | dxfproto::ImportStepImport
    ) {
        info.ImportRows = 0;
    }
    // 按步骤从 TaskMeta Summary 取对应总量（字节或冲突行数）。
    info.Total = match info.Step {
        dxfproto::ImportStepImport | dxfproto::ImportStepWriteAndIngest => {
            task_meta.Summary.IngestSummary.Bytes
        }
        dxfproto::ImportStepEncodeAndSort => task_meta.Summary.EncodeSummary.Bytes,
        dxfproto::ImportStepMergeSort => task_meta.Summary.MergeSummary.Bytes,
        dxfproto::ImportStepCollectConflicts => task_meta.Summary.CollectConflictsSummary.RowCnt,
        dxfproto::ImportStepConflictResolution => task_meta.Summary.ResolveConflictsSummary.RowCnt,
        _ => 0,
    };
    if let Some(time) = latest_time.filter(|time| *time != SystemTime::UNIX_EPOCH) {
        info.UpdateTime = Some(convertToMySQLTime(time, location)?);
    }
    Ok(info)
}

/// 按 Go convertToMySQLTime 的顺序先截断秒以下精度，再转为目标时区的 DATETIME。
pub(crate) fn convertToMySQLTime(
    time: SystemTime,
    location: chrono_tz::Tz,
) -> Result<types::time::Time, errors::SharedError> {
    let utc: chrono::DateTime<chrono::Utc> = time.into();
    let truncated = types::time::TruncateFrac(utc.naive_utc(), 0)
        .map_err(|error| errors::New(error.to_string()))?;
    let utc = chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(truncated, chrono::Utc);
    let local = utc.with_timezone(&location);
    Ok(types::time::NewTime(
        types::time::FromGoTime(local),
        mysql::r#type::TypeDatetime,
        0,
    ))
}

/// 查询指定 job 的最后更新时间。
pub fn GetJobLastUpdateTime(
    provider: &dyn RuntimeInfoProvider,
    job_id: i64,
) -> Result<types::time::Time, errors::SharedError> {
    match provider.GetJobLastUpdateTime(&TaskKey(job_id))? {
        Some(time) => convertToMySQLTime(time, chrono_tz::UTC),
        None => Ok(types::time::ZeroTime),
    }
}

/// 由 job ID 生成 IMPORT INTO 任务键。
pub fn TaskKey(job_id: i64) -> String {
    taskkey::ForJob(job_id)
}

/// 共享的任务提交服务句柄。
pub type SharedTaskSubmissionService = Arc<dyn TaskSubmissionService>;
/// 共享的运行时信息提供者句柄。
pub type SharedRuntimeInfoProvider = Arc<dyn RuntimeInfoProvider>;

/// 速度滑动窗口时长（秒）。
pub fn speed_window() -> Duration {
    execute::SubtaskSpeedUpdateInterval
}
