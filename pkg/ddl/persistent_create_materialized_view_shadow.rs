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

//! Go onCreateMaterializedViewShadow on the normal worker transaction.
use crate::job_worker::JobExecutionContext;
use astersql_meta::TransactionMutator;
use astersql_meta_model::{
    SchemaState,
    group_3::{Job, JobState},
};

fn cancel(job: &mut Job, error: impl ToString) -> String {
    job.state = JobState::Cancelled;
    error.to_string()
}
fn invalid(job: &mut Job, reason: &str) -> String {
    cancel(
        job,
        astersql_util_dbterror::ErrInvalidDDLJob
            .GenWithStackByArgs(&[
                format!("create materialized view shadow table: {reason}").into()
            ]),
    )
}

pub fn step(context: &mut dyn JobExecutionContext, job: &mut Job) -> Result<i64, String> {
    let (table, fk_check) = crate::persistent_create_table::decode_optional(job)?;
    let mut table = table.ok_or_else(|| invalid(job, "invalid shadow metadata"))?;
    let source_id = table
        .MaterializedViewShadow
        .as_ref()
        .ok_or_else(|| invalid(job, "invalid shadow metadata"))?
        .SourceMViewID;
    if table.MaterializedView.is_some()
        || table.MaterializedViewLog.is_some()
        || table.View.is_some()
        || table.Sequence.is_some()
    {
        return Err(invalid(
            job,
            "shadow table must be a protected physical table",
        ));
    }
    if source_id == 0 {
        return Err(invalid(job, "invalid source materialized view id"));
    }
    context.with_transaction(&mut |txn| {
        let meta = TransactionMutator::new(txn);
        if meta.get_database(job.schema_id)?.is_none() {
            return Err(cancel(
                job,
                format!(
                    "[schema:1049]Unknown database '(Schema ID {})'",
                    job.schema_id
                ),
            ));
        }
        let source = meta.get_table(job.schema_id, source_id)?.ok_or_else(|| {
            cancel(
                job,
                format!(
                    "[schema:1146]Table '(Schema ID {}).(Table ID {})' doesn't exist",
                    job.schema_id, source_id
                ),
            )
        })?;
        if source.MaterializedView.is_none() {
            return Err(cancel(
                job,
                astersql_util_dbterror::ErrWrongObject.GenWithStackByArgs(&[
                    job.schema_name.clone().into(),
                    source.Name.O.clone().into(),
                    "MATERIALIZED VIEW".into(),
                ]),
            ));
        }
        if source.State != SchemaState::Public {
            return Err(cancel(
                job,
                astersql_util_dbterror::ErrInvalidDDLState
                    .GenWithStackByArgs(&["table".into(), source.State.to_string().into()]),
            ));
        }
        Ok(Vec::new())
    })?;
    crate::persistent_create_table::create_table(context, job, &mut table, fk_check)?;
    let mut version = 0;
    context.with_transaction(&mut |txn| {
        let mut meta = TransactionMutator::new(txn);
        version = meta.gen_schema_version()?;
        meta.set_create_table_schema_diff(job, version, !table.ForeignKeys.is_empty())?;
        Ok(Vec::new())
    })?;
    crate::persistent_create_table::save_args(job, &table)?;
    job.finish_table_job(
        JobState::Done,
        SchemaState::Public,
        version,
        std::sync::Arc::new(table),
    );
    Ok(version)
}
