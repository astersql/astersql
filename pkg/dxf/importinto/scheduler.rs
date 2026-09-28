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

// Import Into 调度器扩展：任务注册、TiKV 导入模式切换、步骤推进与摘要更新。
//
// 调度器（scheduler）在 DXF 中驱动分布式 Import Into 作业的状态机。
// 本模块在任务运行时向 PD 注册租约、按需把 TiKV 切到导入模式以加速 ingest，
// 并按 local sort / global sort 两条流水线决定下一步（step）。
// TiKV 是分布式 KV 存储；导入模式会放松部分在线事务约束以提升批量写入吞吐。

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use astersql_dxf_framework_dxfmetric as dxfmetric;
use astersql_dxf_framework_proto as framework_proto;
use astersql_dxf_framework_scheduler as framework_scheduler;
use astersql_dxf_framework_storage as framework_storage;
use astersql_errors as errors;
use astersql_executor_importer as importer;
use astersql_parser_ast::misc::redact_url;
use astersql_util_sqlexec as sqlexec;

use crate::metrics::metricsManager;
use crate::proto::{CollectConflictsStepMeta, TaskMeta};

/// 发出警告日志时最多展示的索引数量上限。
pub const warningIndexCount: usize = 32;
/// 向 PD 注册导入任务的租约 TTL（10 分钟）。
pub const registerTaskTTL: Duration = Duration::from_secs(10 * 60);
/// 刷新任务租约的最小间隔（3 分钟）。
pub const refreshTaskTTLInterval: Duration = Duration::from_secs(3 * 60);
/// 单次注册/关闭调用的超时。
pub const registerTimeout: Duration = Duration::from_secs(5);
/// 切换 TiKV 导入模式的最小间隔（5 分钟），避免频繁抖动。
pub const defaultSwitchTiKVModeInterval: Duration = Duration::from_secs(5 * 60);

/// 将调度框架任务快照映射到 IMPORT INTO 使用的 Go 协议任务。
/// 框架接口尚未保留 SchedulerID 和各更新时间，调度回调所需的
/// ID、状态、步骤、资源限制、keyspace、meta 与错误均逐项复制。
pub fn frameworkTaskToImportTask(
    task: &framework_scheduler::Task,
) -> Result<framework_proto::Task, errors::SharedError> {
    if task.base.task_type != framework_proto::ImportInto {
        return Err(errors::New("task type is not ImportInto"));
    }
    let modifications = task
        .modifications
        .iter()
        .map(|modification| {
            let kind = match modification.kind.as_str() {
                framework_proto::ModifyRequiredSlots => framework_proto::ModifyRequiredSlots,
                framework_proto::ModifyMaxNodeCount => framework_proto::ModifyMaxNodeCount,
                framework_proto::ModifyBatchSize => framework_proto::ModifyBatchSize,
                framework_proto::ModifyMaxWriteSpeed => framework_proto::ModifyMaxWriteSpeed,
                _ => return Err(errors::New("unknown task modification")),
            };
            Ok(framework_proto::Modification {
                Type: kind,
                To: modification.to,
            })
        })
        .collect::<Result<Vec<_>, errors::SharedError>>()?;
    Ok(framework_proto::Task {
        TaskBase: framework_proto::TaskBase {
            ID: task.base.id,
            Key: task.base.key.clone(),
            Type: framework_proto::ImportInto,
            State: task.base.state,
            Step: task.base.step,
            Priority: task.base.priority,
            RequiredSlots: task.base.required_slots,
            TargetScope: task.base.target_scope.clone(),
            CreateTime: task.base.create_time,
            MaxNodeCount: task.base.max_node_count,
            ExtraParams: framework_proto::ExtraParams {
                ManualRecovery: task.base.extra_params.manual_recovery,
                PauseOnKVDiskFull: task.base.extra_params.pause_on_kv_disk_full,
                MaxRuntimeSlots: task.base.extra_params.max_runtime_slots,
                TargetSteps: task.base.extra_params.target_steps.clone(),
                PrepareMode: task.base.extra_params.prepare_mode,
            },
            Keyspace: task.base.keyspace.clone(),
        },
        SchedulerID: String::new(),
        StartTime: std::time::SystemTime::UNIX_EPOCH,
        StateUpdateTime: std::time::SystemTime::UNIX_EPOCH,
        Meta: task.meta.clone(),
        Error: task.error.as_ref().map(ToString::to_string),
        ModifyParam: framework_proto::ModifyParam {
            PrevState: task.previous_state,
            Modifications: modifications,
        },
    })
}

/// 以真实 SQLExecutor 驱动 importer 作业 SQL，绑定参数保持原始类型。
pub struct ImportJobSqlSession<'a> {
    pub context: sqlexec::context::Context,
    pub executor: &'a mut dyn sqlexec::SQLExecutor,
}

fn sql_job_arguments(arguments: Vec<importer::JobValue>) -> Vec<Box<dyn std::any::Any>> {
    arguments
        .into_iter()
        .map(|value| match value {
            importer::JobValue::Null => {
                Box::new(sqlexec::types::Datum::default()) as Box<dyn std::any::Any>
            }
            importer::JobValue::Int(value) => Box::new(value) as Box<dyn std::any::Any>,
            importer::JobValue::UInt(value) => Box::new(value) as Box<dyn std::any::Any>,
            importer::JobValue::String(value) => Box::new(value) as Box<dyn std::any::Any>,
            importer::JobValue::Bytes(value) => Box::new(value) as Box<dyn std::any::Any>,
        })
        .collect()
}

struct ImportJobSqlRow(sqlexec::chunk::Row);

impl importer::ImportJobRow for ImportJobSqlRow {
    fn IsNull(&self, index: usize) -> bool {
        self.0.IsNull(index)
    }
    fn Int64(&self, index: usize) -> Result<i64, String> {
        Ok(self.0.GetInt64(index))
    }
    fn String(&self, index: usize) -> Result<String, String> {
        Ok(self.0.GetString(index))
    }
    fn Time(&self, index: usize) -> Result<astersql_types::time::Time, String> {
        Ok(self.0.GetTime(index))
    }
}

impl importer::ImportJobExecutor for ImportJobSqlSession<'_> {
    fn ExecuteInternal(
        &mut self,
        sql: &str,
        arguments: Vec<importer::JobValue>,
    ) -> Result<(), String> {
        sqlexec::ExecSQL(
            &self.context,
            self.executor,
            sql,
            sql_job_arguments(arguments),
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
    }

    fn QueryInternal(
        &mut self,
        sql: &str,
        arguments: Vec<importer::JobValue>,
        expected_columns: usize,
    ) -> Result<Vec<Box<dyn importer::ImportJobRow>>, String> {
        let rows = sqlexec::ExecSQL(
            &self.context,
            self.executor,
            sql,
            sql_job_arguments(arguments),
        )
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
        rows.into_iter()
            .map(|row| {
                if row.Len() != expected_columns {
                    return Err(format!(
                        "expected {expected_columns} columns, got {}",
                        row.Len()
                    ));
                }
                Ok(Box::new(ImportJobSqlRow(row)) as Box<dyn importer::ImportJobRow>)
            })
            .collect()
    }
}

/// DXF TaskManager 会话的 SQL 适配；backend 绑定生产 Session 时直接执行 SQL。
pub struct ImportJobStorageSession {
    pub executor: framework_storage::SQLExecutor,
}

fn storage_job_arguments(arguments: Vec<importer::JobValue>) -> Vec<framework_storage::Value> {
    arguments
        .into_iter()
        .map(|value| match value {
            importer::JobValue::Null => framework_storage::Value::Null,
            importer::JobValue::Int(value) => framework_storage::Value::Int(value),
            importer::JobValue::UInt(value) => framework_storage::Value::U64(value),
            importer::JobValue::String(value) => framework_storage::Value::String(value),
            importer::JobValue::Bytes(value) => framework_storage::Value::Bytes(value),
        })
        .collect()
}

struct ImportJobStorageRow(framework_storage::chunk::Row);
impl importer::ImportJobRow for ImportJobStorageRow {
    fn IsNull(&self, index: usize) -> bool {
        self.0.IsNull(index)
    }
    fn Int64(&self, index: usize) -> Result<i64, String> {
        Ok(self.0.GetInt64(index))
    }
    fn String(&self, index: usize) -> Result<String, String> {
        Ok(self.0.GetString(index))
    }
    fn Time(&self, index: usize) -> Result<astersql_types::time::Time, String> {
        let instant = self.0.GetTime(index).0;
        let utc: chrono::DateTime<chrono::Utc> = instant.into();
        Ok(astersql_types::time::NewTime(
            astersql_types::time::FromGoTime(utc.with_timezone(&chrono_tz::UTC)),
            12,
            6,
        ))
    }
}

impl importer::ImportJobExecutor for ImportJobStorageSession {
    fn ExecuteInternal(
        &mut self,
        sql: &str,
        arguments: Vec<importer::JobValue>,
    ) -> Result<(), String> {
        self.executor
            .execute(sql.into(), storage_job_arguments(arguments))
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
    fn QueryInternal(
        &mut self,
        sql: &str,
        arguments: Vec<importer::JobValue>,
        expected_columns: usize,
    ) -> Result<Vec<Box<dyn importer::ImportJobRow>>, String> {
        self.executor
            .execute(sql.into(), storage_job_arguments(arguments))
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|row| {
                if row.0.len() != expected_columns {
                    return Err(format!(
                        "expected {expected_columns} columns, got {}",
                        row.0.len()
                    ));
                }
                Ok(Box::new(ImportJobStorageRow(row)) as Box<dyn importer::ImportJobRow>)
            })
            .collect()
    }
}

/// 借用 DXF 会话更新导入作业，归还会话由 TaskManager 保证。
pub fn withImportJobSession(
    manager: &framework_storage::TaskManager,
    operation: impl FnOnce(&mut ImportJobStorageSession) -> Result<(), String>,
) -> Result<(), errors::SharedError> {
    manager
        .WithNewSession(|session| {
            let mut adapter = ImportJobStorageSession {
                executor: session.GetSQLExecutor(),
            };
            operation(&mut adapter).map_err(framework_storage::Error::new)
        })
        .map_err(|error| errors::New(error.to_string()))
}

/// 导入作业的系统表更新按 Go scheduler 的 3/6/12/24/30 秒退避重试。
pub fn withImportJobSessionRetry(
    context: &framework_scheduler::Context,
    manager: &framework_storage::TaskManager,
    mut operation: impl FnMut(&mut ImportJobStorageSession) -> Result<(), String>,
) -> Result<(), errors::SharedError> {
    retryImportSQL(context, || {
        manager.WithNewSession(|session| {
            let mut adapter = ImportJobStorageSession {
                executor: session.GetSQLExecutor(),
            };
            operation(&mut adapter).map_err(framework_storage::Error::new)
        })
    })
}

fn retryImportSQL(
    context: &framework_scheduler::Context,
    mut operation: impl FnMut() -> Result<(), framework_storage::Error>,
) -> Result<(), errors::SharedError> {
    let mut last_error = None;
    for retry in 0..framework_scheduler::RETRY_SQL_TIMES {
        let result = operation();
        match result {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error.to_string());
                context
                    .wait(Duration::from_secs((3_u64 << retry.min(4)).min(30)))
                    .map_err(|error| errors::New(error.to_string()))?;
            }
        }
    }
    Err(errors::New(last_error.unwrap_or_default()))
}

struct ImportStatsStore {
    executor: framework_storage::SQLExecutor,
    start_ts: u64,
}

impl astersql_statistics_handle_storage::SqlStore for ImportStatsStore {
    fn start_ts(&self) -> Result<u64, astersql_statistics_handle_storage::Error> {
        Ok(self.start_ts)
    }
    fn execute(
        &self,
        sql: &str,
    ) -> Result<
        Vec<astersql_statistics_handle_storage::Row>,
        astersql_statistics_handle_storage::Error,
    > {
        self.executor
            .execute(sql.to_owned(), Vec::new())
            .map_err(|error| astersql_statistics_handle_storage::Error(error.to_string()))?;
        Ok(Vec::new())
    }
}

/// 与 FinishJob 同一事务内写入导入行数增量；调用方按 Go 语义忽略统计失败。
pub fn FlushImportStatsProduction(
    session: &framework_storage::sessionctx::Context,
    meta: &TaskMeta,
) -> Result<(), errors::SharedError> {
    let table = meta
        .Plan
        .TableInfo
        .as_ref()
        .ok_or_else(|| errors::New("import task plan has no table info"))?;
    let store = ImportStatsStore {
        executor: session.GetSQLExecutor(),
        start_ts: session
            .TxnStartTS()
            .map_err(|error| errors::New(error.to_string()))?,
    };
    astersql_statistics_handle_storage::update_stats_meta(
        &store,
        store.start_ts,
        &[astersql_statistics_handle_storage::DeltaUpdate {
            table_id: table.ID,
            delta: astersql_statistics_handle_storage::TableDelta {
                count: meta.Summary.ImportedRows,
                delta: meta.Summary.ImportedRows,
            },
            is_locked: false,
        }],
    )
    .map_err(|error| errors::New(error.to_string()))
}

/// Classic 导入前在一致性事务中检查目标表行记录（分区由 SQL 层遍历）。
pub fn ProductionCheckImportTableEmpty(
    manager: framework_storage::TaskManager,
) -> Arc<dyn Fn(&TaskMeta) -> Result<(), errors::SharedError> + Send + Sync> {
    Arc::new(move |meta| {
        let table = meta
            .Plan
            .TableInfo
            .as_ref()
            .ok_or_else(|| errors::New("import task plan has no table info"))?;
        let schema = meta.Plan.DBName.replace('`', "``");
        let name = table.Name.O.replace('`', "``");
        manager
            .WithNewTxn((), |session| {
                let rows = session.GetSQLExecutor().execute(
                    format!("select 1 from `{schema}`.`{name}` limit 1"),
                    Vec::new(),
                )?;
                if rows.is_empty() {
                    Ok(())
                } else {
                    Err(framework_storage::Error::new("target table is not empty"))
                }
            })
            .map_err(|error| errors::New(error.to_string()))
    })
}

/// 导入作业表的 Go JSON 字段编码。
pub struct ImportJobJsonCodec;

impl importer::ImportJobCodec for ImportJobJsonCodec {
    fn EncodeParameters(&self, parameters: &importer::ImportParameters) -> Result<Vec<u8>, String> {
        let mut value = serde_json::Map::new();
        value.insert(
            "file-location".into(),
            parameters.FileLocation.clone().into(),
        );
        value.insert("format".into(), parameters.Format.clone().into());
        if !parameters.ColumnsAndVars.is_empty() {
            value.insert(
                "columns-and-vars".into(),
                parameters.ColumnsAndVars.clone().into(),
            );
        }
        if !parameters.SetClause.is_empty() {
            value.insert("set-clause".into(), parameters.SetClause.clone().into());
        }
        if !parameters.Options.is_empty() {
            value.insert(
                "options".into(),
                serde_json::to_value(&parameters.Options).map_err(|error| error.to_string())?,
            );
        }
        serde_json::to_vec(&value).map_err(|error| error.to_string())
    }
    fn DecodeParameters(&self, bytes: &[u8]) -> Result<importer::ImportParameters, String> {
        let value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        let field = |name: &str| {
            value
                .get(name)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_owned()
        };
        let options = value
            .get("options")
            .and_then(serde_json::Value::as_object)
            .map(|object| {
                object
                    .iter()
                    .map(|(name, value)| {
                        (
                            name.clone(),
                            value
                                .as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| value.to_string()),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(importer::ImportParameters {
            ColumnsAndVars: field("columns-and-vars"),
            SetClause: field("set-clause"),
            FileLocation: field("file-location"),
            Format: field("format"),
            Options: options,
        })
    }
    fn EncodeSummary(&self, summary: &importer::Summary) -> Result<Vec<u8>, String> {
        let step = |value: &importer::StepSummary| {
            let mut fields = serde_json::Map::new();
            if value.Bytes != 0 {
                fields.insert("input-bytes".into(), value.Bytes.into());
            }
            if value.RowCnt != 0 {
                fields.insert("input-rows".into(), value.RowCnt.into());
            }
            serde_json::Value::Object(fields)
        };
        let mut value = serde_json::Map::new();
        value.insert("encode-summary".into(), step(&summary.EncodeSummary));
        value.insert("merge-summary".into(), step(&summary.MergeSummary));
        value.insert("ingest-summary".into(), step(&summary.IngestSummary));
        value.insert(
            "collect-conflicts-summary".into(),
            step(&summary.CollectConflictsSummary),
        );
        value.insert(
            "resolve-conflicts-summary".into(),
            step(&summary.ResolveConflictsSummary),
        );
        if summary.ImportedRows != 0 {
            value.insert("row-count".into(), summary.ImportedRows.into());
        }
        if summary.ConflictRowCnt != 0 {
            value.insert("conflict-row-count".into(), summary.ConflictRowCnt.into());
        }
        if summary.TooManyConflicts {
            value.insert("too-many-conflicts".into(), true.into());
        }
        serde_json::to_vec(&value).map_err(|error| error.to_string())
    }
    fn DecodeSummary(&self, bytes: &[u8]) -> Result<importer::Summary, String> {
        let value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        let step = |name: &str| importer::StepSummary {
            Bytes: value
                .get(name)
                .and_then(|step| step.get("input-bytes"))
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0),
            RowCnt: value
                .get(name)
                .and_then(|step| step.get("input-rows"))
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0),
        };
        Ok(importer::Summary {
            EncodeSummary: step("encode-summary"),
            MergeSummary: step("merge-summary"),
            IngestSummary: step("ingest-summary"),
            CollectConflictsSummary: step("collect-conflicts-summary"),
            ResolveConflictsSummary: step("resolve-conflicts-summary"),
            ImportedRows: value
                .get("row-count")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0),
            ConflictRowCnt: value
                .get("conflict-row-count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            TooManyConflicts: value
                .get("too-many-conflicts")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        })
    }
}

/// 准备阶段所需的外部服务，与执行侧复用相同的 controller/importer 边界。
pub struct ImportSchedulerServices {
    pub ControllerServices: Arc<dyn Fn() -> importer::LoadDataControllerServices + Send + Sync>,
    pub ResourceCalculatorWithContext: Option<
        Arc<
            dyn Fn(
                    &framework_scheduler::Context,
                    Arc<dyn importer::ImportResourceCalculator>,
                ) -> Arc<dyn importer::ImportResourceCalculator>
                + Send
                + Sync,
        >,
    >,
    pub ImporterService: Arc<dyn Fn() -> Arc<dyn importer::TableImporterService> + Send + Sync>,
    pub KVCodec: Vec<u8>,
    /// 可预绑定表与外部存储；生产环境省略时由元数据/URI 构造。
    pub Table: Option<Arc<dyn Fn() -> Arc<dyn astersql_table::Table> + Send + Sync>>,
    pub SortStore: Option<astersql_objstore::storage::StorageRef>,
    /// 在经典内核下以新事务检查目标表；与 Go DDL 边界同形。
    pub CheckImportTableEmpty:
        Option<Arc<dyn Fn(&TaskMeta) -> Result<(), errors::SharedError> + Send + Sync>>,
    /// 提供真实 KV/对象存储与 controller 运行时给物理规划器。
    pub PlanContext: Arc<
        dyn Fn(&framework_proto::Task, framework_proto::Step) -> crate::planner::PlanCtx
            + Send
            + Sync,
    >,
    /// FinishJob 同一事务中的 stats 更新；失败为非致命，对齐 Go。
    pub FlushStatsBestEffort: Option<
        Arc<
            dyn Fn(
                    &framework_storage::sessionctx::Context,
                    &TaskMeta,
                ) -> Result<(), errors::SharedError>
                + Send
                + Sync,
        >,
    >,
}

impl ImportSchedulerServices {
    /// 与节点侧 4088/4090 的编码运行时共用 controller/importer 工厂。
    pub fn FromEncodeRuntime(
        runtime: Arc<crate::encode_and_sort_operator::ConfiguredEncodeSortRuntime>,
        kv_codec: Vec<u8>,
        plan_context: Arc<
            dyn Fn(&framework_proto::Task, framework_proto::Step) -> crate::planner::PlanCtx
                + Send
                + Sync,
        >,
    ) -> Self {
        let importer_runtime = runtime.clone();
        let controller_runtime = runtime.clone();
        Self {
            ControllerServices: Arc::new(move || {
                let mut services = (controller_runtime.ControllerServices)();
                services.StorageFactory = Arc::new(importer::HostImportStorageFactory {
                    CloudFactory: services.StorageFactory,
                });
                services.SizeEstimator = Arc::new(importer::HostImportSizeEstimator {
                    ParquetEstimator: services.SizeEstimator,
                });
                services
            }),
            ResourceCalculatorWithContext: Some(Arc::new(|context, sampler| {
                Arc::new(importer::HostImportResourceCalculator {
                    Handle: astersql_dxf_framework_handle::Context::from_cancellation_flag(
                        context.cancellation_flag(),
                    ),
                    SampleService: sampler,
                })
            })),
            ImporterService: Arc::new(move || {
                Arc::new(importer::HostTableImporterService {
                    Host: importer_runtime.importer_service(),
                })
            }),
            KVCodec: kv_codec,
            Table: None,
            SortStore: None,
            CheckImportTableEmpty: None,
            PlanContext: plan_context,
            FlushStatsBestEffort: None,
        }
    }
}

struct PreparedController(importer::LoadDataController);
impl Deref for PreparedController {
    type Target = importer::LoadDataController;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl DerefMut for PreparedController {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl Drop for PreparedController {
    fn drop(&mut self) {
        self.0.Close();
    }
}

struct OwnedSortStore(astersql_objstore::storage::StorageRef);
impl Drop for OwnedSortStore {
    fn drop(&mut self) {
        self.0.Close();
    }
}

/// 按 Go OnPrepare 顺序启动作业、发现/校验文件、计算资源并持久化 chunk map。
pub fn prepareImportTask(
    context: &framework_scheduler::Context,
    manager: &framework_storage::TaskManager,
    task: &mut framework_proto::Task,
    services: &ImportSchedulerServices,
) -> Result<(), errors::SharedError> {
    let mut task_meta = TaskMeta::Unmarshal(&task.Meta)?;
    withImportJobSessionRetry(context, manager, |executor| {
        importer::StartJob(executor, task_meta.JobID, importer::JobStepPreparing)
    })?;
    let table_info = task_meta
        .Plan
        .TableInfo
        .as_ref()
        .ok_or_else(|| errors::New("import task plan has no table info"))?;
    let table: Arc<dyn astersql_table::Table> = if let Some(table) = &services.Table {
        table()
    } else {
        Arc::from(
            astersql_table::BuildTableFromMeta(table_info)
                .map_err(|error| errors::New(error.to_string()))?
                .ok_or_else(|| errors::New("table metadata factory is not installed"))?,
        )
    };
    let args = importer::ASTArgsFromStmt(&task_meta.Stmt).map_err(errors::New)?;
    let mut controller_services = (services.ControllerServices)();
    if let Some(resource_factory) = &services.ResourceCalculatorWithContext {
        controller_services.ResourceCalculator =
            resource_factory(context, controller_services.ResourceCalculator);
    }
    let mut controller = PreparedController(
        importer::NewLoadDataController(
            task_meta.Plan.clone(),
            table,
            args,
            controller_services,
            Vec::new(),
        )
        .map_err(errors::New)?,
    );
    let was_auto = controller.Plan.Format == importer::DataFormatAuto;
    let store_context =
        astersql_objstore_storeapi::Context::from_cancellation_flag(context.cancellation_flag());
    controller
        .InitDataFiles(&store_context)
        .map_err(errors::New)?;
    controller
        .CheckImportDataSizeWithLimit(
            astersql_config_deploymode::IsStarter(),
            astersql_config::get_global_config()
                .starter_params
                .max_import_data_size,
        )
        .map_err(errors::New)?;
    controller
        .CalResourceParams(&services.KVCodec)
        .map_err(errors::New)?;
    withImportJobSessionRetry(context, manager, |executor| {
        importer::UpdateJobPreparedInfo(
            executor,
            &ImportJobJsonCodec,
            task_meta.JobID,
            controller.Plan.TotalFileSize,
            &controller.Plan.Format,
        )
    })?;
    if was_auto && controller.Plan.Format != importer::DataFormatCSV {
        controller
            .Plan
            .CheckNonCSVFormatOptions()
            .map_err(errors::New)?;
    }
    let execute_node_count = controller.Plan.MaxNodeCnt.max(0) as usize;
    controller.SetExecuteNodeCnt(execute_node_count);
    let chunks = controller
        .PopulateChunks((services.ImporterService)().as_ref())
        .map_err(errors::New)?;
    let prepared_path = astersql_ingestor_globalsort::PreparedMetaPath(task.ID);
    let prepared = crate::proto::PreparedMeta {
        ChunkMap: chunks,
        ..Default::default()
    };
    let sort_uri = if controller.Plan.CloudStorageURI.is_empty() {
        "."
    } else {
        &controller.Plan.CloudStorageURI
    };
    let sort_context =
        astersql_objstore::storage::Context::from_cancellation_flag(context.cancellation_flag());
    let (store, _owned_store) = if let Some(store) = &services.SortStore {
        (store.clone(), None)
    } else {
        let store = astersql_objstore::storage::NewFromURL(&sort_context, sort_uri)
            .map_err(|error| errors::New(error.to_string()))?;
        (store.clone(), Some(OwnedSortStore(store)))
    };
    store
        .WriteFile(&sort_context, &prepared_path, &prepared.Marshal()?)
        .map_err(|error| errors::New(error.to_string()))?;
    task_meta.Plan = controller.Plan.clone();
    task_meta.PreparedMetaExternalPath = prepared_path;
    updateMeta(task, &task_meta)?;
    task.RequiredSlots = task_meta.Plan.ThreadCnt as i32;
    task.MaxNodeCount = task_meta.Plan.MaxNodeCnt;
    Ok(())
}

/// Go OnNextSubtasksBatch：先推进 job/收集前序 meta，再生成物理子任务并写回摘要。
pub fn nextImportSubtasksBatch(
    context: &framework_scheduler::Context,
    scheduler: &importScheduler,
    manager: &framework_storage::TaskManager,
    handle: &dyn framework_scheduler::TaskHandle,
    task: &mut framework_proto::Task,
    exec_ids: &[String],
    next_step: framework_proto::Step,
    services: &ImportSchedulerServices,
) -> Result<Vec<Vec<u8>>, errors::SharedError> {
    let mut task_meta = TaskMeta::Unmarshal(&task.Meta)?;
    if astersql_config_kerneltype::IsClassic() && task.Step == framework_proto::StepInit {
        if let Some(check) = &services.CheckImportTableEmpty {
            check(&task_meta)?;
        } else {
            ProductionCheckImportTableEmpty(manager.clone())(&task_meta)?;
        }
    }
    let mut previous = HashMap::new();
    let mut fetch = |step| -> Result<(), errors::SharedError> {
        previous.insert(
            step,
            handle
                .previous_subtask_metas(task.ID, step)
                .map_err(|error| errors::New(error.to_string()))?,
        );
        Ok(())
    };
    let advance = |step: &str| {
        withImportJobSessionRetry(context, manager, |executor| {
            importer::Job2Step(executor, task_meta.JobID, step)
        })
    };
    match next_step {
        framework_proto::ImportStepImport | framework_proto::ImportStepEncodeAndSort => {
            scheduler
                .metrics
                .bytes_counter
                .with_label_values(&[astersql_lightning_metric::STATE_TOTAL_RESTORE])
                .inc_by(task_meta.Plan.TotalFileSize as f64);
            let job_step = if scheduler.GlobalSort {
                importer::JobStepGlobalSorting
            } else {
                importer::JobStepImporting
            };
            if task.ExtraParams.PrepareMode == framework_proto::PrepareModeRequired {
                advance(job_step)?;
            } else {
                withImportJobSessionRetry(context, manager, |executor| {
                    importer::StartJob(executor, task_meta.JobID, job_step)
                })?;
            }
            if task_meta
                .Plan
                .TableInfo
                .as_ref()
                .is_some_and(|table| importer::GetNumOfIndexGenKV(table) > warningIndexCount)
            {
                dxfmetric::InitDistTaskMetrics()
                    .ScheduleEventCounter
                    .with_label_values(&[task.ID.to_string().as_str(), dxfmetric::EventTooManyIdx])
                    .inc();
            }
        }
        framework_proto::ImportStepMergeSort => fetch(framework_proto::ImportStepEncodeAndSort)?,
        framework_proto::ImportStepWriteAndIngest => {
            fetch(framework_proto::ImportStepEncodeAndSort)?;
            fetch(framework_proto::ImportStepMergeSort)?;
            advance(importer::JobStepImporting)?;
        }
        framework_proto::ImportStepCollectConflicts
        | framework_proto::ImportStepConflictResolution => {
            fetch(framework_proto::ImportStepEncodeAndSort)?;
            fetch(framework_proto::ImportStepMergeSort)?;
            fetch(framework_proto::ImportStepWriteAndIngest)?;
            advance(importer::JobStepResolvingConflicts)?;
        }
        framework_proto::ImportStepPostProcess => {
            scheduler.switchTiKV2NormalMode(task);
            advance(importer::JobStepValidating)?;
            fetch(getStepOfEncode(scheduler.GlobalSort))?;
            fetch(framework_proto::ImportStepCollectConflicts)?;
        }
        framework_proto::StepDone => return Ok(Vec::new()),
        _ => return Err(errors::New(format!("unknown step {}", task.Step))),
    }
    let mut plan_ctx = (services.PlanContext)(task, next_step);
    plan_ctx.TaskID = task.ID;
    plan_ctx.PreviousSubtaskMetas = previous;
    plan_ctx.GlobalSort = scheduler.GlobalSort;
    plan_ctx.NextTaskStep = next_step;
    plan_ctx.ExecuteNodesCnt = if astersql_config_kerneltype::IsNextGen() {
        task.MaxNodeCount
    } else {
        exec_ids.len() as i32
    };
    plan_ctx.ThreadCnt = task.GetRuntimeSlots();
    if task_meta.ChunkMap.is_empty() && task_meta.PreparedMetaExternalPath.is_empty() {
        plan_ctx.ControllerServices = Some(services.ControllerServices.clone());
        plan_ctx.ImporterService = Some((services.ImporterService)());
    }
    let mut logical = crate::planner::LogicalPlan::default();
    logical.FromTaskMeta(&task.Meta)?;
    let physical = logical.ToPhysicalPlan(plan_ctx.clone())?;
    let metas = physical.ToSubtaskMetas(&plan_ctx, next_step)?;
    let post_process = if next_step == framework_proto::ImportStepPostProcess {
        let summaries = handle
            .previous_subtask_summaries(task.ID, getStepOfEncode(task_meta.Plan.IsGlobalSort()))
            .map_err(|error| errors::New(error.to_string()))?;
        let row_counts: Vec<i64> = summaries.iter().map(|summary| summary.row_count).collect();
        let conflict_metas = handle
            .previous_subtask_metas(task.ID, framework_proto::ImportStepCollectConflicts)
            .map_err(|error| errors::New(error.to_string()))?
            .iter()
            .map(|bytes| {
                let value: serde_json::Value = serde_json::from_slice(bytes)
                    .map_err(|error| errors::New(error.to_string()))?;
                Ok(CollectConflictsStepMeta {
                    ConflictedRowCount: value
                        .get("conflicted-row-count")
                        .and_then(serde_json::Value::as_i64)
                        .unwrap_or(0),
                    TooManyConflictsFromIndex: value
                        .get("too-many-conflicts-from-index")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                    ..Default::default()
                })
            })
            .collect::<Result<Vec<_>, errors::SharedError>>()?;
        Some((row_counts, conflict_metas))
    } else {
        None
    };
    updateTaskSummary(
        task,
        &mut task_meta,
        next_step,
        &logical.summary,
        post_process
            .as_ref()
            .map(|(rows, conflicts)| PostProcessSummaryInput {
                encoded_row_counts: rows,
                conflict_metas: conflicts,
            }),
    )?;
    if next_step == framework_proto::ImportStepMergeSort && !metas.is_empty() {
        dxfmetric::InitDistTaskMetrics()
            .ScheduleEventCounter
            .with_label_values(&[task.ID.to_string().as_str(), dxfmetric::EventMergeSort])
            .inc();
    }
    Ok(metas)
}

/// Go OnDone：先尽力恢复表模式，再按取消/失败/成功更新作业状态。
pub fn doneImportTask(
    context: &framework_scheduler::Context,
    scheduler: &importScheduler,
    manager: &framework_storage::TaskManager,
    task: &framework_proto::Task,
    flush_stats: &dyn Fn(
        &framework_storage::sessionctx::Context,
        &TaskMeta,
    ) -> Result<(), errors::SharedError>,
) -> Result<(), errors::SharedError> {
    let task_meta = TaskMeta::Unmarshal(&task.Meta)?;
    if astersql_config_kerneltype::IsClassic() {
        resetClassicTableMode(manager, &task_meta);
    }
    if task.State == framework_proto::TaskStateReverting {
        scheduler.switchTiKV2NormalMode(task);
        scheduler.unregisterTaskWithContext(context, task.ID);
        if task.Error.as_deref().is_some_and(|error| {
            framework_scheduler::IsCancelledErr(&framework_scheduler::SchedulerError::new(error))
        }) {
            return withImportJobSessionRetry(context, manager, |executor| {
                importer::CancelJob(executor, task_meta.JobID)
            });
        }
        return withImportJobSessionRetry(context, manager, |executor| {
            importer::FailJob(
                executor,
                &ImportJobJsonCodec,
                task_meta.JobID,
                task.Error.as_deref().unwrap_or(""),
                Some(&task_meta.Summary),
            )
        });
    }
    scheduler.unregisterTaskWithContext(context, task.ID);
    retryImportSQL(context, || {
        manager.WithNewTxn((), |session| {
            let _ = flush_stats(&session, &task_meta);
            let mut executor = ImportJobStorageSession {
                executor: session.GetSQLExecutor(),
            };
            importer::FinishJob(
                &mut executor,
                &ImportJobJsonCodec,
                task_meta.JobID,
                Some(&task_meta.Summary),
            )
            .map_err(framework_storage::Error::new)
        })
    })
}

/// Go OnDone resets the table to normal before deciding job status. Cleanup
/// remains a fallback, so a DDL failure does not hide the original task state.
pub(crate) fn resetClassicTableMode(
    manager: &framework_storage::TaskManager,
    task_meta: &TaskMeta,
) {
    if let Some(table) = &task_meta.Plan.TableInfo {
        if task_meta.Plan.DBID != 0 && table.ID != 0 {
            let _ = manager.WithNewTxn((), |session| {
                session.AlterTableModeForNormal(task_meta.Plan.DBID, table.ID)
            });
        }
    }
}

/// 将 IMPORT INTO 状态机接到 DXF Scheduler.Extension。
pub struct ImportSchedulerExtension {
    pub scheduler: Arc<importScheduler>,
    pub manager: framework_storage::TaskManager,
    pub services: Arc<ImportSchedulerServices>,
}

/// 以真实框架任务、参数和服务构造可调度的 IMPORT INTO BaseScheduler。
pub fn NewImportSchedulerWithServices(
    task: framework_scheduler::Task,
    param: framework_scheduler::Param,
    runtime: Arc<dyn ImportSchedulerRuntime>,
    manager: framework_storage::TaskManager,
    services: Arc<ImportSchedulerServices>,
) -> Result<Arc<framework_scheduler::BaseScheduler>, errors::SharedError> {
    let imported = frameworkTaskToImportTask(&task)?;
    let selected_manager = GetImportJobTaskManager(runtime.as_ref(), &manager)?;
    let scheduler = Arc::new(importScheduler::new(runtime, &imported)?);
    let extension = Arc::new(ImportSchedulerExtension {
        scheduler,
        manager: selected_manager,
        services,
    });
    Ok(Arc::new(framework_scheduler::BaseScheduler::new(
        task, param, extension,
    )))
}

struct FailedImportScheduler {
    task: framework_scheduler::Task,
    error: framework_scheduler::SchedulerError,
    extension: Arc<ImportSchedulerExtension>,
}
impl framework_scheduler::Scheduler for FailedImportScheduler {
    fn init(&self) -> framework_scheduler::Result<()> {
        self.extension.scheduler.Close(self.task.base.id);
        Err(self.error.clone())
    }
    fn schedule_once(&self) -> framework_scheduler::Result<bool> {
        Err(self.error.clone())
    }
    fn close(&self) {
        self.extension.scheduler.Close(self.task.base.id);
    }
    fn task(&self) -> framework_scheduler::Task {
        self.task.clone()
    }
    fn extension(&self) -> Arc<dyn framework_scheduler::Extension> {
        self.extension.clone()
    }
}

/// 注册生产 IMPORT INTO factory；坏 meta 在 Init 报错并注销 metrics，与 Go 时机一致。
pub fn RegisterImportSchedulerFactoryWithServices(
    runtime: Arc<dyn ImportSchedulerRuntime>,
    manager: framework_storage::TaskManager,
    services: Arc<ImportSchedulerServices>,
) {
    framework_scheduler::RegisterSchedulerFactory(
        framework_proto::ImportInto,
        Arc::new(move |task, param| {
            match NewImportSchedulerWithServices(
                task.clone(),
                param,
                runtime.clone(),
                manager.clone(),
                services.clone(),
            ) {
                Ok(scheduler) => scheduler as Arc<dyn framework_scheduler::Scheduler>,
                Err(error) => {
                    let scheduler = Arc::new(importScheduler::withoutMeta(
                        runtime.clone(),
                        task.base.id,
                        task.base.keyspace.clone(),
                    ));
                    let extension = Arc::new(ImportSchedulerExtension {
                        scheduler,
                        manager: manager.clone(),
                        services: services.clone(),
                    });
                    Arc::new(FailedImportScheduler {
                        task,
                        error: framework_scheduler::SchedulerError::new(error.to_string()),
                        extension,
                    }) as Arc<dyn framework_scheduler::Scheduler>
                }
            }
        }),
    );
}

impl framework_scheduler::Extension for ImportSchedulerExtension {
    fn on_tick(&self, task: &framework_scheduler::Task) {
        self.on_tick_with_context(&framework_scheduler::Context::default(), task);
    }
    fn on_tick_with_context(
        &self,
        context: &framework_scheduler::Context,
        task: &framework_scheduler::Task,
    ) {
        if let Ok(task) = frameworkTaskToImportTask(task) {
            self.scheduler.OnTickWithContext(context, &task);
        }
    }
    fn on_next_subtasks_batch(
        &self,
        handle: &dyn framework_scheduler::TaskHandle,
        task: &mut framework_scheduler::Task,
        exec_ids: &[String],
        next_step: framework_scheduler::Step,
    ) -> framework_scheduler::Result<Vec<Vec<u8>>> {
        self.on_next_subtasks_batch_with_context(
            &framework_scheduler::Context::default(),
            handle,
            task,
            exec_ids,
            next_step,
        )
    }
    fn on_next_subtasks_batch_with_context(
        &self,
        context: &framework_scheduler::Context,
        handle: &dyn framework_scheduler::TaskHandle,
        task: &mut framework_scheduler::Task,
        exec_ids: &[String],
        next_step: framework_scheduler::Step,
    ) -> framework_scheduler::Result<Vec<Vec<u8>>> {
        let mut imported = frameworkTaskToImportTask(task)
            .map_err(|error| framework_scheduler::SchedulerError::new(error.to_string()))?;
        let metas = nextImportSubtasksBatch(
            context,
            self.scheduler.as_ref(),
            &self.manager,
            handle,
            &mut imported,
            exec_ids,
            next_step,
            self.services.as_ref(),
        )
        .map_err(|error| framework_scheduler::SchedulerError::new(error.to_string()))?;
        task.base.required_slots = imported.RequiredSlots;
        task.base.max_node_count = imported.MaxNodeCount;
        task.meta = imported.Meta;
        Ok(metas)
    }
    fn on_done(
        &self,
        handle: &dyn framework_scheduler::TaskHandle,
        task: &mut framework_scheduler::Task,
    ) -> framework_scheduler::Result<()> {
        self.on_done_with_context(&framework_scheduler::Context::default(), handle, task)
    }
    fn on_done_with_context(
        &self,
        context: &framework_scheduler::Context,
        _handle: &dyn framework_scheduler::TaskHandle,
        task: &mut framework_scheduler::Task,
    ) -> framework_scheduler::Result<()> {
        let imported = frameworkTaskToImportTask(task)
            .map_err(|error| framework_scheduler::SchedulerError::new(error.to_string()))?;
        doneImportTask(
            context,
            self.scheduler.as_ref(),
            &self.manager,
            &imported,
            self.services
                .FlushStatsBestEffort
                .as_deref()
                .unwrap_or(&FlushImportStatsProduction),
        )
        .map_err(|error| framework_scheduler::SchedulerError::new(error.to_string()))
    }
    fn eligible_instances(
        &self,
        task: &framework_scheduler::Task,
    ) -> framework_scheduler::Result<Vec<String>> {
        let imported = frameworkTaskToImportTask(task)
            .map_err(|error| framework_scheduler::SchedulerError::new(error.to_string()))?;
        self.scheduler
            .GetEligibleInstances(&imported)
            .map_err(|error| framework_scheduler::SchedulerError::new(error.to_string()))
    }
    fn is_retryable_error(&self, error: &framework_scheduler::SchedulerError) -> bool {
        IsImportSchedulerRetryableError(error)
    }
    fn next_step(&self, task: &framework_scheduler::TaskBase) -> framework_scheduler::Step {
        self.scheduler.GetNextStep(&framework_proto::TaskBase {
            ID: task.id,
            Key: task.key.clone(),
            Type: framework_proto::ImportInto,
            State: task.state,
            Step: task.step,
            Priority: task.priority,
            RequiredSlots: task.required_slots,
            TargetScope: task.target_scope.clone(),
            CreateTime: task.create_time,
            MaxNodeCount: task.max_node_count,
            ExtraParams: framework_proto::ExtraParams::default(),
            Keyspace: task.keyspace.clone(),
        })
    }
    fn on_prepare(
        &self,
        handle: &dyn framework_scheduler::TaskHandle,
        task: &mut framework_scheduler::Task,
    ) -> framework_scheduler::Result<()> {
        self.on_prepare_with_context(&framework_scheduler::Context::default(), handle, task)
    }
    fn on_prepare_with_context(
        &self,
        context: &framework_scheduler::Context,
        _handle: &dyn framework_scheduler::TaskHandle,
        task: &mut framework_scheduler::Task,
    ) -> framework_scheduler::Result<()> {
        let mut imported = frameworkTaskToImportTask(task)
            .map_err(|error| framework_scheduler::SchedulerError::new(error.to_string()))?;
        prepareImportTask(
            context,
            &self.manager,
            &mut imported,
            self.services.as_ref(),
        )
        .map_err(|error| framework_scheduler::SchedulerError::new(error.to_string()))?;
        task.base.required_slots = imported.RequiredSlots;
        task.base.max_node_count = imported.MaxNodeCount;
        task.meta = imported.Meta;
        Ok(())
    }
    fn modify_meta(
        &self,
        old_meta: &[u8],
        _modifications: &[framework_scheduler::Modification],
    ) -> framework_scheduler::Result<Vec<u8>> {
        Ok(old_meta.to_vec())
    }
}

/// Framework error text retains Go normalized errors' `[class:code]` prefix.
pub fn IsImportSchedulerRetryableError(error: &framework_scheduler::SchedulerError) -> bool {
    if error
        .0
        .contains("failed to get cross keyspace session pool")
    {
        return true;
    }
    let mut common = astersql_lightning_common::CommonError::new("", error.0.clone());
    if let Some(prefix) = error
        .0
        .strip_prefix('[')
        .and_then(|text| text.split_once(']'))
    {
        if let Some((_, code)) = prefix.0.split_once(':') {
            common.Code = code.parse::<u16>().ok();
        }
    }
    astersql_lightning_common::IsRetryableError(Some(&common))
}

/// PD 侧任务注册句柄：可一次注册并在结束时关闭。
pub trait TaskRegistration: Send {
    /// 在超时内完成一次注册刷新。
    fn register_once(&mut self, timeout: Duration) -> Result<(), errors::SharedError>;
    fn register_once_with_context(
        &mut self,
        context: &framework_scheduler::Context,
        timeout: Duration,
    ) -> Result<(), errors::SharedError> {
        if context.is_cancelled() {
            return Err(errors::New("context canceled"));
        }
        self.register_once(timeout)
    }
    /// 关闭注册并释放租约。
    fn close(&mut self, timeout: Duration) -> Result<(), errors::SharedError>;
    fn close_with_context(
        &mut self,
        context: &framework_scheduler::Context,
        timeout: Duration,
    ) -> Result<(), errors::SharedError> {
        if context.is_cancelled() {
            return Err(errors::New("context canceled"));
        }
        self.close(timeout)
    }
}

/// PD 注册与 TiKV 模式切换的运行时边界。
/// Runtime boundary for PD registration and TiKV mode switching.
pub trait ImportSchedulerRuntime: Send + Sync {
    /// Go `kv.IsUserKS(Store())`：用户 keyspace 的作业 SQL 使用系统会话池。
    fn is_user_keyspace(&self) -> bool {
        false
    }
    /// Go `TaskRuntime.SysSessionPool()`；只有用户 keyspace 需要。
    fn system_session_pool(&self) -> Option<framework_storage::util::SessionPool> {
        None
    }
    /// 为指定任务创建带 TTL 的注册句柄。
    fn new_task_registration(
        &self,
        task_id: i64,
        ttl: Duration,
    ) -> Result<Box<dyn TaskRegistration>, errors::SharedError>;
    /// 将 TiKV 切到导入模式。
    fn switch_to_import_mode(&self) -> Result<(), errors::SharedError>;
    /// 将 TiKV 切回正常在线模式。
    fn switch_to_normal_mode(&self) -> Result<(), errors::SharedError>;
}

pub fn GetImportJobTaskManager(
    runtime: &dyn ImportSchedulerRuntime,
    default_manager: &framework_storage::TaskManager,
) -> Result<framework_storage::TaskManager, errors::SharedError> {
    if !runtime.is_user_keyspace() {
        return Ok(default_manager.clone());
    }
    let pool = runtime
        .system_session_pool()
        .ok_or_else(|| errors::New("failed to get cross keyspace session pool"))?;
    Ok(framework_storage::NewTaskManager(pool))
}

/// 单个导入任务的注册状态（上次刷新时间与注册句柄）。
pub struct taskInfo {
    pub taskID: i64,
    lastRegisterTime: Option<Instant>,
    taskRegister: Option<Box<dyn TaskRegistration>>,
}

impl taskInfo {
    /// 创建尚未注册的任务信息。
    fn new(task_id: i64) -> Self {
        Self {
            taskID: task_id,
            lastRegisterTime: None,
            taskRegister: None,
        }
    }

    /// 按刷新间隔续租；失败故意非致命，时间戳仍前进，留给租约内两次重试。
    /// Refresh failure is deliberately non-fatal. The timestamp still moves
    /// forward, leaving two retries within the ten-minute lease, like Go.
    pub fn register(&mut self, runtime: &dyn ImportSchedulerRuntime) {
        self.registerWithContext(&framework_scheduler::Context::default(), runtime);
    }
    pub fn registerWithContext(
        &mut self,
        context: &framework_scheduler::Context,
        runtime: &dyn ImportSchedulerRuntime,
    ) {
        if context.is_cancelled() {
            return;
        }
        // 未到刷新间隔则跳过，避免过度打 PD。
        if self
            .lastRegisterTime
            .is_some_and(|last| last.elapsed() < refreshTaskTTLInterval)
        {
            return;
        }
        // 懒创建注册句柄；创建失败则本轮直接返回。
        if self.taskRegister.is_none() {
            let Ok(registration) = runtime.new_task_registration(self.taskID, registerTaskTTL)
            else {
                return;
            };
            self.taskRegister = Some(registration);
        }
        let _ = self
            .taskRegister
            .as_mut()
            .expect("task registration initialized")
            .register_once_with_context(context, registerTimeout);
        self.lastRegisterTime = Some(Instant::now());
    }

    /// 关闭并丢弃注册句柄。
    pub fn close(&mut self) {
        self.closeWithContext(&framework_scheduler::Context::default());
    }
    pub fn closeWithContext(&mut self, context: &framework_scheduler::Context) {
        if let Some(mut registration) = self.taskRegister.take() {
            let _ = registration.close_with_context(context, registerTimeout);
        }
    }
}

/// Import Into 调度扩展：持有运行时、排序模式与 TiKV 模式/注册状态。
pub struct importScheduler {
    runtime: Arc<dyn ImportSchedulerRuntime>,
    pub metrics: Arc<astersql_lightning_metric::Common>,
    /// 是否走 global sort（云端排序）流水线。
    pub GlobalSort: bool,
    modeSwitch: Mutex<Option<Instant>>,
    taskInfoMap: Mutex<HashMap<i64, taskInfo>>,
    currTaskID: AtomicI64,
    disableTiKVImportMode: AtomicBool,
    /// 任务所属 keyspace（多租户隔离域）。
    pub taskKS: String,
}

impl importScheduler {
    fn withoutMeta(
        runtime: Arc<dyn ImportSchedulerRuntime>,
        task_id: i64,
        keyspace: String,
    ) -> Self {
        let metrics = metricsManager.get_or_create_metrics(task_id);
        Self {
            runtime,
            metrics,
            GlobalSort: false,
            modeSwitch: Mutex::new(None),
            taskInfoMap: Mutex::new(HashMap::new()),
            currTaskID: AtomicI64::new(0),
            disableTiKVImportMode: AtomicBool::new(false),
            taskKS: keyspace,
        }
    }
    /// 从任务元数据构造调度扩展，并注册任务级 metrics。
    pub fn new(
        runtime: Arc<dyn ImportSchedulerRuntime>,
        task: &framework_proto::Task,
    ) -> Result<Self, errors::SharedError> {
        let task_meta = TaskMeta::Unmarshal(&task.Meta)?;
        let mut scheduler = Self::withoutMeta(runtime, task.ID, task.Keyspace.clone());
        scheduler.GlobalSort = task_meta.Plan.IsGlobalSort();
        Ok(scheduler)
    }

    /// 任务结束时注销 metrics。
    pub fn Close(&self, task_id: i64) {
        metricsManager.unregister(task_id);
    }

    /// 运行中周期回调：维持 TiKV 导入模式并刷新 PD 任务注册。
    pub fn OnTick(&self, task: &framework_proto::Task) {
        self.OnTickWithContext(&framework_scheduler::Context::default(), task);
    }
    pub fn OnTickWithContext(
        &self,
        context: &framework_scheduler::Context,
        task: &framework_proto::Task,
    ) {
        if task.State != framework_proto::TaskStateRunning {
            return;
        }
        self.switchTiKVMode(task);
        self.registerTaskWithContext(context, task.ID);
    }

    /// 当前步骤是否正在向 TiKV 写入（local import 或 write-and-ingest）。
    pub fn isImporting2TiKV(&self, task: &framework_proto::Task) -> bool {
        matches!(
            task.Step,
            framework_proto::ImportStepImport | framework_proto::ImportStepWriteAndIngest
        )
    }

    /// 在导入相关步骤按节流间隔切换到 TiKV 导入模式。
    pub fn switchTiKVMode(&self, task: &framework_proto::Task) {
        self.updateCurrentTask(task);
        // 计划禁用导入模式，或不在写入 TiKV 的步骤时跳过。
        if self.disableTiKVImportMode.load(Ordering::Acquire) || !self.isImporting2TiKV(task) {
            return;
        }
        let mut last_switch = self
            .modeSwitch
            .lock()
            .expect("TiKV mode-switch mutex poisoned");
        if last_switch.is_some_and(|last| last.elapsed() < defaultSwitchTiKVModeInterval) {
            return;
        }
        let _ = self.runtime.switch_to_import_mode();
        *last_switch = Some(Instant::now());
    }

    /// 将 TiKV 切回正常模式，并清空上次切换时间戳。
    pub fn switchTiKV2NormalMode(&self, task: &framework_proto::Task) {
        self.updateCurrentTask(task);
        if self.disableTiKVImportMode.load(Ordering::Acquire) {
            return;
        }
        let mut last_switch = self
            .modeSwitch
            .lock()
            .expect("TiKV mode-switch mutex poisoned");
        let _ = self.runtime.switch_to_normal_mode();
        *last_switch = None;
    }

    /// 确保任务已登记并按间隔续租。
    pub fn registerTask(&self, task_id: i64) {
        self.registerTaskWithContext(&framework_scheduler::Context::default(), task_id);
    }
    pub fn registerTaskWithContext(&self, context: &framework_scheduler::Context, task_id: i64) {
        let mut tasks = self
            .taskInfoMap
            .lock()
            .expect("import task registration mutex poisoned");
        tasks
            .entry(task_id)
            .or_insert_with(|| taskInfo::new(task_id))
            .registerWithContext(context, self.runtime.as_ref());
    }

    /// 移除任务注册并关闭句柄。
    pub fn unregisterTask(&self, task_id: i64) {
        self.unregisterTaskWithContext(&framework_scheduler::Context::default(), task_id);
    }
    pub fn unregisterTaskWithContext(&self, context: &framework_scheduler::Context, task_id: i64) {
        let mut tasks = self
            .taskInfoMap
            .lock()
            .expect("import task registration mutex poisoned");
        if let Some(mut task) = tasks.remove(&task_id) {
            task.closeWithContext(context);
        }
    }

    /// 缓存当前任务 ID；任务变化时根据 Plan 更新是否禁用 TiKV 导入模式。
    pub fn updateCurrentTask(&self, task: &framework_proto::Task) {
        // 同一任务重复更新时跳过反序列化。
        if self.currTaskID.swap(task.ID, Ordering::AcqRel) == task.ID {
            return;
        }
        if let Ok(task_meta) = TaskMeta::Unmarshal(&task.Meta) {
            // RaftKV2 或不允许导入模式时关闭模式切换。
            self.disableTiKVImportMode.store(
                task_meta.Plan.DisableTiKVImportMode || task_meta.Plan.IsRaftKV2,
                Ordering::Release,
            );
        }
    }

    /// 从任务元数据解析可调度实例列表（`host:port` / `[ipv6]:port`）。
    pub fn GetEligibleInstances(
        &self,
        task: &framework_proto::Task,
    ) -> Result<Vec<String>, errors::SharedError> {
        let task_meta = TaskMeta::Unmarshal(&task.Meta)?;
        Ok(task_meta
            .EligibleInstances
            .iter()
            .map(|server| joinHostPort(&server.ip, server.listening_port))
            .collect())
    }

    /// 按 local / global sort 状态机返回下一步。
    pub fn GetNextStep(&self, task: &framework_proto::TaskBase) -> framework_proto::Step {
        match task.Step {
            framework_proto::StepInit | framework_proto::StepPrepared => {
                if self.GlobalSort {
                    framework_proto::ImportStepEncodeAndSort
                } else {
                    framework_proto::ImportStepImport
                }
            }
            framework_proto::ImportStepEncodeAndSort => framework_proto::ImportStepMergeSort,
            framework_proto::ImportStepMergeSort => framework_proto::ImportStepWriteAndIngest,
            framework_proto::ImportStepWriteAndIngest => {
                framework_proto::ImportStepCollectConflicts
            }
            framework_proto::ImportStepCollectConflicts => {
                framework_proto::ImportStepConflictResolution
            }
            framework_proto::ImportStepImport | framework_proto::ImportStepConflictResolution => {
                framework_proto::ImportStepPostProcess
            }
            _ => framework_proto::StepDone,
        }
    }

    /// 元数据修改占位：当前原样返回旧 meta。
    pub fn ModifyMeta(
        &self,
        old_meta: Vec<u8>,
        _modifications: &[framework_proto::Modification],
    ) -> Vec<u8> {
        old_meta
    }
}

/// 将结构化 `TaskMeta` 序列化写回任务。
pub fn updateMeta(
    task: &mut framework_proto::Task,
    task_meta: &TaskMeta,
) -> Result<(), errors::SharedError> {
    task.Meta = task_meta.Marshal()?;
    Ok(())
}

/// 根据是否 global sort 返回编码阶段对应的第一步。
pub fn getStepOfEncode(global_sort: bool) -> framework_proto::Step {
    if global_sort {
        framework_proto::ImportStepEncodeAndSort
    } else {
        framework_proto::ImportStepImport
    }
}

/// 后处理摘要输入：编码行数与收集冲突步骤的持久化元数据。
pub struct PostProcessSummaryInput<'a> {
    /// 各编码子任务产出的行数。
    pub encoded_row_counts: &'a [i64],
    /// collect-conflicts 子任务写回的元数据切片。
    pub conflict_metas: &'a [CollectConflictsStepMeta],
}

/// 按即将进入的步骤写入逻辑计划摘要；后处理时由子任务结果推导最终导入行数。
/// Store the logical-plan summary for a step and, for post-process, derive the
/// final imported-row count from persisted subtask results.
pub fn updateTaskSummary(
    task: &mut framework_proto::Task,
    task_meta: &mut TaskMeta,
    next_step: framework_proto::Step,
    step_summary: &importer::StepSummary,
    post_process: Option<PostProcessSummaryInput<'_>>,
) -> Result<(), errors::SharedError> {
    match next_step {
        framework_proto::ImportStepEncodeAndSort | framework_proto::ImportStepImport => {
            task_meta.Summary.EncodeSummary = step_summary.clone();
        }
        framework_proto::ImportStepMergeSort => {
            task_meta.Summary.MergeSummary = step_summary.clone();
        }
        framework_proto::ImportStepWriteAndIngest => {
            task_meta.Summary.IngestSummary = step_summary.clone();
        }
        framework_proto::ImportStepCollectConflicts => {
            task_meta.Summary.CollectConflictsSummary = step_summary.clone();
        }
        framework_proto::ImportStepConflictResolution => {
            task_meta.Summary.ResolveConflictsSummary = step_summary.clone();
        }
        framework_proto::ImportStepPostProcess => {
            let input = post_process
                .ok_or_else(|| errors::New("post-process summary input is required"))?;
            // 先按编码行数求和，global sort 再扣除冲突行。
            for row_count in input.encoded_row_counts {
                task_meta.Summary.ImportedRows =
                    task_meta.Summary.ImportedRows.wrapping_add(*row_count);
            }
            if task_meta.Plan.IsGlobalSort() {
                let mut conflicted_rows = 0_u64;
                for meta in input.conflict_metas {
                    // 索引侧冲突过多时只打标，不计入可扣减冲突行数。
                    if meta.TooManyConflictsFromIndex {
                        task_meta.Summary.TooManyConflicts = true;
                        continue;
                    }
                    conflicted_rows = conflicted_rows.wrapping_add(meta.ConflictedRowCount as u64);
                }
                task_meta.Summary.ImportedRows = task_meta
                    .Summary
                    .ImportedRows
                    .wrapping_sub(conflicted_rows as i64);
                task_meta.Summary.ConflictRowCnt = conflicted_rows;
            }
        }
        _ => {}
    }
    updateMeta(task, task_meta)
}

/// 清理语句文本并脱敏路径/云存储 URI 后写回任务 meta。
pub fn redactSensitiveInfo(task: &mut framework_proto::Task, task_meta: &mut TaskMeta) {
    task_meta.Stmt.clear();
    task_meta.Plan.Path = redact_url(&task_meta.Plan.Path);
    if !task_meta.Plan.CloudStorageURI.is_empty() {
        task_meta.Plan.CloudStorageURI = redact_url(&task_meta.Plan.CloudStorageURI);
    }
    let _ = updateMeta(task, task_meta);
}

/// 将 host 与 port 拼成地址；含冒号的 host 按 IPv6 加方括号。
fn joinHostPort(host: &str, port: u32) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}
