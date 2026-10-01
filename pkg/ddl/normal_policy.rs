// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

//! Normal scheduler upgrade admission, operating on the durable SQL job queue.
use crate::job_worker::DurableJobSession;
use crate::table_mode::DdlJobPolicy;
use astersql_meta_model::group_3::{
    AdminCommandOperator, JOB_PAUSE_REASON_KV_DISK_FULL, Job, JobState,
};
use std::sync::Arc;

pub struct NormalDdlJobPolicy {
    pub state: Arc<dyn astersql_ddl_serverstate::Syncer>,
    pub context: astersql_ddl_serverstate::SyncContext,
    pub owner_id: String,
    /// Service-owned snapshot, fixed for the entire queue dispatch round.
    pub round_upgrading: Option<Arc<std::sync::atomic::AtomicBool>>,
}
impl DdlJobPolicy for NormalDdlJobPolicy {
    fn runnable(&mut self, session: &mut dyn DurableJobSession, job: &Job) -> Result<bool, String> {
        // The normal service refreshes this cache once per scheduling round,
        // before publishing the owner operation and loading the durable queue.
        let upgrading = self.round_upgrading.as_ref().map_or_else(
            || self.state.is_upgrading_state(),
            |state| state.load(std::sync::atomic::Ordering::Acquire),
        );
        if upgrading {
            if job.is_paused() {
                return Ok(false);
            }
            if job.is_pausing()
                || job
                    .get_involving_schema_info()
                    .iter()
                    .any(|info| astersql_meta_metadef::IsSystemRelatedDB(&info.database))
            {
                return Ok(true);
            }
            match system_command(session, job.id, false) {
                Ok(()) => {}
                Err(error) => {
                    let runnable = error.starts_with("[ddl:8260]");
                    eprintln!("pause normal DDL job {} during upgrade: {error}", job.id);
                    if runnable {
                        return Ok(true);
                    }
                }
            }
            return Ok(false);
        }
        if job.is_paused_by_system() {
            if job.has_pause_reason(JOB_PAUSE_REASON_KV_DISK_FULL) {
                return Ok(false);
            }
            system_command(session, job.id, true)?;
            return Err(format!("system paused job:{} need to be resumed", job.id));
        }
        Ok(!job.is_paused())
    }
    fn error_limit(&self) -> i64 {
        astersql_sessionctx_vardef::GetDDLErrorCountLimit()
    }
    fn mdl_owner(&self) -> Option<String> {
        astersql_sessionctx_vardef::IsMDLEnabled().then(|| self.owner_id.clone())
    }
}

// ddl.go processJobs runs each administrative command in its own transaction,
// retrying the whole read/change/commit sequence three times on commit failure.
fn system_command(
    session: &mut dyn DurableJobSession,
    id: i64,
    resume: bool,
) -> Result<(), String> {
    let mut commit_error = None;
    for _ in 0..3 {
        session.begin()?;
        let outcome = (|| {
            let rows = session.query(
                &format!(
                    "select job_meta from mysql.tidb_ddl_job where job_id={id} order by job_id"
                ),
                "admin-get-job",
            )?;
            let Some(raw) = rows.first().and_then(|row| row.first()) else {
                return Ok(Err(format!("[ddl:8224]DDL job {id} not found")));
            };
            let mut job = astersql_meta::decode_go_history_job(raw.as_bytes())
                .map_err(|error| error.to_string())?;
            let command = if resume {
                resume_system_job(&mut job)
            } else {
                pause_system_job(&mut job)
            };
            if command.is_ok() {
                let bytes = astersql_meta::encode_go_ddl_job(&mut job, false)?;
                let encoded = bytes
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>();
                if let Err(error) = session.query(
                    &format!(
                        "update mysql.tidb_ddl_job set job_meta=X'{encoded}' where job_id={id}"
                    ),
                    "admin-update-job",
                ) {
                    return Ok(Err(error));
                }
            }
            Ok(command)
        })();
        let command = match outcome {
            Ok(command) => command,
            Err(error) => {
                session.rollback();
                return Err(error);
            }
        };
        match session.commit() {
            Ok(()) => return command,
            Err(error) => {
                session.rollback();
                commit_error = Some(error);
            }
        }
    }
    Err(commit_error.expect("three failed DDL command commits"))
}
fn pause_system_job(job: &mut Job) -> Result<(), String> {
    if job.is_pausing() || job.is_paused() {
        return Err(format!("[ddl:8262]DDL job {} is paused", job.id));
    }
    if !job.is_pausable() {
        return Err(format!(
            "[ddl:8260]Cannot pause DDL job {}, state [{}] or schema state [{}]",
            job.id, job.state, job.schema_state
        ));
    }
    job.state = JobState::Pausing;
    job.admin_operator = AdminCommandOperator::System;
    Ok(())
}
fn resume_system_job(job: &mut Job) -> Result<(), String> {
    if !job.is_resumable() || job.admin_operator != AdminCommandOperator::System {
        return Err(format!("[ddl:8261]Cannot resume DDL job {}", job.id));
    }
    job.state = JobState::Queueing;
    job.clear_pause_reason();
    job.error = None;
    job.clear_resume_reason();
    Ok(())
}
