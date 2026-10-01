// Copyright 2026 AsterSQL.
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

//! Go mview_worker.go onAlterMaterializedViewAttributes, in the normal worker transaction.
use crate::job_worker::JobExecutionContext;
use astersql_meta_model::{
    SchemaState,
    group_3::{Job, JobState},
};
pub fn step(context: &mut dyn JobExecutionContext, job: &mut Job) -> Result<i64, String> {
    let args = astersql_meta_model::group_2::GetAlterMaterializedViewAttributesArgs(job).map_err(
        |error| {
            job.state = JobState::Cancelled;
            error.to_string()
        },
    )?;
    let mut output = None;
    let mut version = 0;
    context.with_transaction(&mut |txn| {
        let mut meta = astersql_meta::TransactionMutator::new(txn);
        let mut table = crate::persistent_actions::public_table(&meta, job)?;
        if table.MaterializedView.is_none() {
            job.state = JobState::Cancelled;
            return Err(astersql_util_dbterror::ErrWrongObject
                .GenWithStackByArgs(&[
                    job.schema_name.clone().into(),
                    job.table_name.clone().into(),
                    "MATERIALIZED VIEW".into(),
                ])
                .to_string());
        }
        if job.multi_schema_info.as_ref().is_some_and(|m| m.revertible) {
            job.mark_non_revertible();
            return Ok(Vec::new());
        }
        let old = table.clone();
        let view = table.MaterializedView.as_mut().unwrap();
        view.AlertWarningSec = args.AlertWarningSec;
        view.AlertOverdueSec = args.AlertOverdueSec;
        view.AlertRefreshFailed = args.AlertRefreshFailed;
        version = crate::persistent_actions::update_version_and_table(&mut meta, job, &mut table)?;
        output = Some((table, old));
        Ok(Vec::new())
    })?;
    if let Some((table, old)) = output {
        crate::persistent_actions::async_notify_event(
            context,
            job,
            -1,
            astersql_ddl_notifier::NewAlterMaterializedViewAttributesEvent(
                Some(Box::new(table.clone())),
                Some(Box::new(old)),
            ),
        )?;
        job.finish_table_job(
            JobState::Done,
            SchemaState::Public,
            version,
            std::sync::Arc::new(table),
        );
    }
    Ok(version)
}
