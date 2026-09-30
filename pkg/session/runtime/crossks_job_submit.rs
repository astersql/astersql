// Copyright 2026 AsterSQL.

//! Durable cross-keyspace AlterTableMode job submission.

use std::sync::Arc;
use std::time::Duration;

use astersql_ddl_bdr as bdr;
use astersql_ddl_jobsubmit as jobsubmit;
use astersql_domain_crossks::{AlterTableModeJob, TableMode};

use super::crossks_session_pool::{
    CrossKSFlashbackGuard, CrossKSJobSessionPool, CrossKSMinJobId, CrossKSSessionPool,
};

struct CrossKSBDRPolicy;

impl jobsubmit::BdrPolicy for CrossKSBDRPolicy {
    fn is_denied(
        &self,
        role: &str,
        job_type: jobsubmit::JobType,
        _args: &jobsubmit::JobArgs,
    ) -> bool {
        let role = match role.to_ascii_lowercase().as_str() {
            "primary" => bdr::ast::BDRRole::Primary,
            "secondary" => bdr::ast::BDRRole::Secondary,
            "none" | "" => bdr::ast::BDRRole::None,
            _ => bdr::ast::BDRRole::Unknown,
        };
        bdr::IsDenied(role, job_type.code() as u8, None)
    }
}

/// Submit Go-compatible table-mode jobs through the real target SQL pool.
pub struct CrossKSJobSubmitter {
    options: jobsubmit::SubmitOptions,
}

impl CrossKSJobSubmitter {
    pub fn new(
        session_pool: Arc<CrossKSSessionPool>,
        guard: Arc<CrossKSFlashbackGuard>,
        min_job_id: Arc<CrossKSMinJobId>,
        server_state: Option<Arc<dyn jobsubmit::ServerState>>,
    ) -> Self {
        Self {
            options: jobsubmit::SubmitOptions {
                session_pool: Arc::new(CrossKSJobSessionPool::new(session_pool)),
                system_table_manager: guard,
                min_job_id_provider: min_job_id,
                server_state,
                bdr_policy: Arc::new(CrossKSBDRPolicy),
                before_insert_with_assigned_ids: None,
                max_retry_count: 5,
                backoff: Arc::new(|attempt| {
                    std::thread::sleep(Duration::from_millis(10 * (attempt as u64 + 1)));
                }),
            },
        }
    }

    pub fn submit_table_mode(&self, job: &mut AlterTableModeJob) -> Result<(), jobsubmit::Error> {
        let mode = match job.target_mode {
            TableMode::Normal => 0,
            TableMode::Import => 1,
            TableMode::Restore => 2,
        };
        let raw_args = serde_json::to_vec(&serde_json::json!({
            "table_mode": mode,
            "schema_id": job.schema_id,
            "table_id": job.table_id,
        }))
        .expect("serialize table-mode DDL arguments");
        let mut spec = jobsubmit::JobSpec {
            job: jobsubmit::Job {
                version: 2,
                schema_id: job.schema_id,
                table_id: job.table_id,
                schema_name: job.schema_name.clone(),
                table_name: job.table_name.clone(),
                job_type: jobsubmit::JobType::AlterTableMode,
                query: job.query.clone(),
                cdc_write_source: job.cdc_write_source,
                sql_mode: job.sql_mode,
                binlog_info_present: true,
                involving_schemas: vec![(job.schema_name.clone(), job.table_name.clone())],
                ..Default::default()
            },
            args: jobsubmit::JobArgs::Opaque(raw_args),
            id_allocated: true,
        };
        jobsubmit::submit_batch(&self.options, std::slice::from_mut(&mut spec))?;
        job.id = spec.job.id;
        Ok(())
    }
}
