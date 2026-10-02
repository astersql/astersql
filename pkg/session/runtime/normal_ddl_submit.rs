// Copyright 2026 AsterSQL.
//! Ordinary DDL submission and durable history waiting on the shared system pool.
use super::system_session::SystemSessionPool;
use astersql_ddl_jobsubmit as submit;
use astersql_meta_model::group_3::{Job, JobState};
use std::sync::Arc;
struct Bdr;
impl submit::BdrPolicy for Bdr {
    fn is_denied(&self, role: &str, tp: submit::JobType, _: &submit::JobArgs) -> bool {
        use astersql_ddl_bdr::ast::BDRRole;
        let role = match role {
            "primary" => BDRRole::Primary,
            "secondary" => BDRRole::Secondary,
            "none" | "" => BDRRole::None,
            _ => BDRRole::Unknown,
        };
        astersql_ddl_bdr::IsDenied(role, tp.code() as u8, None)
    }
}
pub(super) fn submit_and_wait(
    pool: &Arc<SystemSessionPool>,
    cancel: &astersql_owner::manager::Context,
    state: Option<Arc<dyn submit::ServerState>>,
    job: &mut Job,
) -> Result<(), String> {
    let manager = astersql_ddl_systable::new_manager(pool.clone());
    let min = Arc::new(astersql_ddl_systable::new_min_job_id_refresher(
        manager.clone(),
    ));
    min.refresh(&astersql_ddl_systable::Context::default());
    let mut options = pool.table_mode_submit_options(manager, min, state);
    options.bdr_policy = Arc::new(Bdr);
    let vars = pool.acquire()?.ddl_session_variables()?;
    let mut spec = submit::JobSpec {
        job: submit::Job {
            version: 2,
            schema_id: job.schema_id,
            table_id: job.table_id,
            schema_name: job.schema_name.clone(),
            table_name: job.table_name.clone(),
            job_type: match job.tp {
                12 => submit::JobType::ModifyColumn,
                14 => submit::JobType::RenameTable,
                47 => submit::JobType::RenameTables,
                tp => submit::JobType::Other(tp as i64),
            },
            query: job.query.clone(),
            cdc_write_source: job.cdc_write_source,
            sql_mode: job.sql_mode,
            session_vars: job.session_vars.clone(),
            reorg_meta: job.reorg_meta.as_ref().map(|meta| {
                serde_json::from_value(
                    serde_json::to_value(meta).expect("reorg metadata serialization"),
                )
                .map(Arc::new)
                .expect("reorg metadata snapshot")
            }),
            binlog_info_present: true,
            need_reorg: job.may_need_reorg(),
            involving_schemas: job
                .involving_schema_info
                .iter()
                .map(|info| (info.database.clone(), info.table.clone()))
                .collect(),
            ..Default::default()
        },
        args: submit::JobArgs::Opaque(job.raw_args.clone()),
        id_allocated: true,
    };
    if spec.job.cdc_write_source == 0 {
        spec.job.cdc_write_source = vars.cdc_write_source;
    }
    if job.tp == 11 {
        let original: serde_json::Value =
            serde_json::from_slice(&job.raw_args).map_err(|e| e.to_string())?;
        let old_ids = original
            .get("old_partition_ids")
            .and_then(|v| v.as_array())
            .map(|ids| ids.iter().filter_map(|id| id.as_i64()).collect())
            .unwrap_or_default();
        spec.job.job_type = submit::JobType::TruncateTable;
        spec.id_allocated = false;
        spec.args = submit::JobArgs::TruncateTable {
            old_partition_ids: old_ids,
            new_table_id: 0,
            new_partition_ids: Vec::new(),
        };
        options.before_insert_with_assigned_ids = Some(Arc::new(move |specs| {
            for spec in specs {
                if let submit::JobArgs::TruncateTable {
                    new_table_id,
                    new_partition_ids,
                    ..
                } = &spec.args
                {
                    let mut value = original.clone();
                    value["new_table_id"] = (*new_table_id).into();
                    value["new_partition_ids"] = serde_json::json!(new_partition_ids);
                    spec.args = submit::JobArgs::Opaque(
                        serde_json::to_vec(&value).expect("Go job args JSON"),
                    );
                }
            }
            None
        }));
    }
    submit::submit_batch(&options, std::slice::from_mut(&mut spec)).map_err(|e| e.to_string())?;
    job.id = spec.job.id;
    loop {
        if cancel.is_cancelled() {
            return Err(format!("DDL waiting cancelled for job {}", job.id));
        }
        if let Some(history) = pool.acquire()?.persistent_history(job.id)? {
            *job = history;
            if job.state == JobState::Synced {
                return Ok(());
            }
            return Err(job
                .error
                .clone()
                .unwrap_or_else(|| format!("DDL job {} ended in {:?}", job.id, job.state)));
        }
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
}
impl super::ConcreteSession {
    pub(super) fn persistent_actions_enabled(&self) -> bool {
        self.domain
            .ddl()
            .is_some_and(|service| service.supports_persistent_actions())
    }
    pub(super) fn submit_normal_action(
        &self,
        database: &str,
        table: &str,
        action: u8,
        args: serde_json::Value,
    ) -> super::SessionResult<()> {
        let info = self
            .domain
            .table_by_name(database, table)
            .map_err(|e| super::SessionError::new(e.to_string()))?;
        let db = self
            .domain
            .info_schema()
            .AllSchemas()
            .into_iter()
            .find(|db| db.name.lower == database.to_ascii_lowercase())
            .ok_or_else(|| super::SessionError::new(format!("unknown database {database}")))?;
        let mode = astersql_parser_mysql::r#const::GetSQLMode(&self.state.borrow().sql_mode)
            .map_err(|e| super::SessionError::new(e.to_string()))?;
        let mut job = Job {
            tp: action,
            schema_id: db.id,
            table_id: info.ID,
            schema_name: database.to_ascii_lowercase(),
            table_name: table.to_ascii_lowercase(),
            sql_mode: mode.0 as u64,
            raw_args: serde_json::to_vec(&args)
                .map_err(|e| super::SessionError::new(e.to_string()))?,
            ..Default::default()
        };
        if action == 12 {
            let new: astersql_meta_model::ColumnInfo =
                serde_json::from_value(args["column"].clone())
                    .map_err(|e| super::SessionError::new(e.to_string()))?;
            let old_name = args["old_column_name"]["L"]
                .as_str()
                .or_else(|| args["old_column_name"]["l"].as_str())
                .unwrap_or("");
            if let Some(old) = info.Columns.iter().find(|c| c.Name.L == old_name) {
                job.need_reorg =
                    !astersql_ddl::persistent_modify_column::no_reorg_data_strict(&info, old, &new);
            }
        }
        if action == 12 {
            for name in [
                astersql_sessionctx_vardef::TiDBAnalyzeVersion,
                astersql_sessionctx_vardef::TiDBEnableDDLAnalyze,
            ] {
                if let Some(value) = self.session_vars.GetSystemVar(name) {
                    job.session_vars.insert(name.to_owned(), value);
                }
            }
            use astersql_meta_model::group_3::{CurrentReorgMetaVersion, DDLReorgMeta};
            let (name, offset) = match *self.time_zone.borrow() {
                super::session::RuntimeTimeZone::Named(zone) => (zone.name().to_owned(), 0),
                super::session::RuntimeTimeZone::Fixed(zone) => {
                    (String::new(), zone.local_minus_utc())
                }
            };
            let mut meta = DDLReorgMeta {
                SQLMode: job.sql_mode,
                Location: Some(Box::new(astersql_meta_model::TimeZoneLocation {
                    name,
                    offset,
                    ..Default::default()
                })),
                ResourceGroupName: self.session_vars.StmtCtx.ResourceGroupName.clone(),
                Version: CurrentReorgMetaVersion,
                UseNewCollate: Some(astersql_util_collate::NewCollationEnabled()),
                ..Default::default()
            };
            if let Some(value) = self
                .session_vars
                .GetSystemVar(astersql_sessionctx_vardef::TiDBDDLReorgWorkerCount)
            {
                meta.SetConcurrency(value.parse().unwrap_or(0));
            }
            if let Some(value) = self
                .session_vars
                .GetSystemVar(astersql_sessionctx_vardef::TiDBDDLReorgBatchSize)
            {
                meta.SetBatchSize(value.parse().unwrap_or(0));
            }
            meta.SetMaxWriteSpeed(astersql_sessionctx_vardef::DDLReorgMaxWriteSpeed.Load());
            if job.need_reorg {
                meta.IsFastReorg = self
                    .domain
                    .global_system_variable(astersql_sessionctx_vardef::TiDBDDLEnableFastReorg)
                    .map(|value| astersql_sessionctx_variable::TiDBOptOn(&value))
                    .unwrap_or_else(|| astersql_sessionctx_vardef::EnableFastReorg.Load());
                meta.IsDistReorg = astersql_sessionctx_vardef::EnableDistTask.Load();
                meta.TargetScope = if astersql_config_kerneltype::IsNextGen() {
                    "dxf_service".to_owned()
                } else {
                    astersql_config::get_global_config()
                        .instance
                        .tidb_service_scope
                        .clone()
                };
                if let Some(value) = self
                    .session_vars
                    .GetSystemVar(astersql_sessionctx_vardef::TiDBMaxDistTaskNodes)
                {
                    meta.MaxNodeCount = value.parse().unwrap_or(0);
                }
                if matches!(
                    job.schema_name.as_str(),
                    "mysql"
                        | "sys"
                        | "information_schema"
                        | "performance_schema"
                        | "metrics_schema"
                ) {
                    meta.IsFastReorg = false;
                    meta.IsDistReorg = false;
                }
                if meta.IsDistReorg && !meta.IsFastReorg {
                    return Err(super::SessionError::new(
                        "[ddl:8200]Unsupported distributed task without fast reorganization",
                    ));
                }
            }
            job.reorg_meta = Some(meta);
        }

        if action == 14 {
            job.schema_id = args
                .get("new_schema_id")
                .and_then(|id| id.as_i64())
                .unwrap_or(job.schema_id);
        }
        self.domain
            .ddl()
            .ok_or_else(|| super::SessionError::new("normal DDL service unavailable"))?
            .submit_persistent_job(&mut job)
            .map_err(super::SessionError::new)?;
        self.domain
            .reload()
            .map_err(|e| super::SessionError::new(e.to_string()))?;
        self.update_self_version_with_retry()
    }
}
