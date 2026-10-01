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

//! Go mview_worker.go: atomic CREATE MATERIALIZED VIEW LOG on the normal worker.
use crate::job_worker::JobExecutionContext;
use astersql_meta::TransactionMutator;
use astersql_meta_model::{
    SchemaState, TableInfo,
    group_3::{Job, JobState},
};
use std::sync::Arc;
fn cancel(job: &mut Job, error: impl ToString) -> String {
    job.state = JobState::Cancelled;
    error.to_string()
}
fn invalid(job: &mut Job, reason: &str) -> String {
    cancel(
        job,
        astersql_util_dbterror::ErrInvalidDDLJob
            .GenWithStackByArgs(&[format!("create materialized view log: {reason}").into()]),
    )
}
fn decode(job: &mut Job) -> Result<TableInfo, String> {
    let args = astersql_meta_model::group_2::GetCreateMaterializedViewLogArgs(job)
        .map_err(|e| cancel(job, e))?;
    let table = args
        .TableInfo
        .ok_or_else(|| invalid(job, "invalid job args"))?;
    let table: TableInfo =
        serde_json::from_value(serde_json::to_value(table).map_err(|e| cancel(job, e))?)
            .map_err(|e| cancel(job, e))?;
    if table.MaterializedViewLog.is_none() {
        return Err(invalid(job, "invalid job args"));
    }
    Ok(table)
}
fn missing_table(job: &Job, id: i64) -> String {
    format!(
        "[schema:1146]Table '(Schema ID {}).(Table ID {})' doesn't exist",
        job.schema_id, id
    )
}
/// No intermediate schema state is committed: purge registration, notifier and
/// both metadata objects participate in the enclosing worker's single transaction.
pub fn step(context: &mut dyn JobExecutionContext, job: &mut Job) -> Result<i64, String> {
    let mut log = decode(job)?;
    if job.state == JobState::Rollingback {
        return rollback(context, job, &log);
    }
    let base_id = log.MaterializedViewLog.as_ref().unwrap().BaseTableID;
    if base_id == 0 {
        return Err(invalid(job, "invalid base table id"));
    }
    let mut base = None;
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
        let table = meta
            .get_table(job.schema_id, base_id)?
            .ok_or_else(|| cancel(job, missing_table(job, base_id)))?;
        if astersql_meta_metadef::IsMemOrSysDB(&job.schema_name.to_lowercase())
            || table.View.is_some()
            || table.Sequence.is_some()
            || table.TempTableType != astersql_meta_model::TempTableNone
            || table.MaterializedView.is_some()
            || table.MaterializedViewShadow.is_some()
            || table.MaterializedViewLog.is_some()
        {
            return Err(cancel(
                job,
                astersql_util_dbterror::ErrWrongObject.GenWithStackByArgs(&[
                    job.schema_name.clone().into(),
                    table.Name.O.clone().into(),
                    "BASE TABLE".into(),
                ]),
            ));
        }
        if table.GetPartitionInfo().is_some() {
            return Err(cancel(
                job,
                astersql_util_dbterror::ErrGeneralUnsupportedDDL.GenWithStackByArgs(&[
                    "CREATE MATERIALIZED VIEW LOG on partition table".into(),
                ]),
            ));
        }
        if table.State != SchemaState::Public {
            return Err(cancel(
                job,
                astersql_util_dbterror::ErrInvalidDDLState.GenWithStack(
                    "table %s is not in public, but %s",
                    &[table.Name.O.clone().into(), table.State.to_string().into()],
                ),
            ));
        }
        if table
            .MaterializedViewBase
            .as_ref()
            .is_some_and(|b| b.MLogID != 0)
        {
            return Err(cancel(
                job,
                format!(
                    "[schema:1050]Table '{}.{}' already exists",
                    job.schema_name, log.Name
                ),
            ));
        }
        base = Some(table);
        Ok(Vec::new())
    })?;
    crate::persistent_create_table::create_table(context, job, &mut log, false)?;
    let mut base = base.ok_or("base table metadata unavailable")?;
    base.MaterializedViewBase
        .get_or_insert_with(Default::default)
        .MLogID = log.ID;
    context.with_transaction(&mut |txn| {
        TransactionMutator::new(txn).update_table(job.schema_id, &mut base)?;
        Ok(Vec::new())
    })?;
    let (next, should_update) = context.derive_create_mlog_schedule(&job.schema_name, &log)?;
    let sql = if should_update {
        format!(
            "INSERT INTO mysql.tidb_mlog_purge_info (MLOG_ID,NEXT_PURGE_UNIX_SECONDS) VALUES ({},{}) ON DUPLICATE KEY UPDATE NEXT_PURGE_UNIX_SECONDS=VALUES(NEXT_PURGE_UNIX_SECONDS)",
            log.ID,
            next.map_or("NULL".into(), |n| n.to_string())
        )
    } else {
        format!(
            "INSERT IGNORE INTO mysql.tidb_mlog_purge_info (MLOG_ID) VALUES ({})",
            log.ID
        )
    };
    if let Err(error) = context.query(&sql, "mlog-purge-info-upsert") {
        if is_missing_purge_table(&error) {
            job.state = JobState::Rollingback;
            return Err(astersql_util_dbterror::ErrInvalidDDLJob.GenWithStackByArgs(&["create materialized view log: required system table mysql.tidb_mlog_purge_info does not exist".into()]).to_string());
        }
        return Err(error);
    }
    let mut version = 0;
    context.with_transaction(&mut |txn| {
        let mut meta = TransactionMutator::new(txn);
        version = meta.gen_schema_version()?;
        meta.set_create_mlog_schema_diff(job, version, &[base.ID], false)?;
        Ok(Vec::new())
    })?;
    crate::persistent_actions::async_notify_event(
        context,
        job,
        -1,
        astersql_ddl_notifier::NewCreateTableEvent(Some(Box::new(log.clone()))),
    )?;
    crate::persistent_create_table::save_args(job, &log)?;
    job.finish_multiple_table_job(
        JobState::Done,
        SchemaState::Public,
        version,
        vec![Arc::new(base), Arc::new(log)],
    );
    Ok(version)
}
fn is_missing_purge_table(error: &str) -> bool {
    (error.contains("1146") && error.contains("tidb_mlog_purge_info"))
        || error == "unknown DML table tidb_mlog_purge_info"
}
fn rollback(
    context: &mut dyn JobExecutionContext,
    job: &mut Job,
    args: &TableInfo,
) -> Result<i64, String> {
    let mut affected = Vec::new();
    context.with_transaction(&mut |txn| {
        let mut meta = TransactionMutator::new(txn);
        let actual = if meta.get_database(job.schema_id)?.is_some() {
            meta.get_table(job.schema_id, job.table_id)?
        } else {
            None
        };
        let dropping = actual.as_ref().unwrap_or(args);
        if let Some(info) = &dropping.MaterializedViewLog {
            if let Some(mut base) = meta.get_table(job.schema_id, info.BaseTableID)? {
                if let Some(b) = &mut base.MaterializedViewBase {
                    if b.MLogID == job.table_id {
                        b.MLogID = 0;
                    }
                    if b.MLogID == 0 && b.MViewIDs.is_empty() {
                        base.MaterializedViewBase = None;
                    }
                }
                meta.update_table(job.schema_id, &mut base)?;
                affected.push(base.ID);
            }
        }
        if actual.is_some() {
            meta.drop_table_and_auto_ids(job.schema_id, job.table_id)?;
        }
        Ok(Vec::new())
    })?;
    if let Err(error) = context.query(
        &format!(
            "DELETE FROM mysql.tidb_mlog_purge_info WHERE MLOG_ID={}",
            job.table_id
        ),
        "mlog-purge-info-delete",
    ) {
        if !is_missing_purge_table(&error) {
            return Err(error);
        }
    }
    job.state = JobState::RollbackDone;
    job.schema_state = SchemaState::None;
    let mut version = 0;
    context.with_transaction(&mut |txn| {
        let mut meta = TransactionMutator::new(txn);
        version = meta.gen_schema_version()?;
        meta.set_create_mlog_schema_diff(job, version, &affected, true)?;
        Ok(Vec::new())
    })?;
    Ok(version)
}
