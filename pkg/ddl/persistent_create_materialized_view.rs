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

//! Go onCreateMaterializedView StateNone on the ordinary worker transaction.
use crate::job_worker::JobExecutionContext;
use astersql_meta::TransactionMutator;
use astersql_meta_model::{
    SchemaState, TableInfo,
    group_3::{Job, JobState, JobVersion},
};
fn cancel(job: &mut Job, error: impl ToString) -> String {
    job.state = JobState::Cancelled;
    error.to_string()
}
fn invalid(job: &mut Job, reason: &str) -> String {
    cancel(
        job,
        astersql_util_dbterror::ErrInvalidDDLJob
            .GenWithStackByArgs(&[format!("create materialized view: {reason}").into()]),
    )
}
fn missing(job: &mut Job, id: i64) -> String {
    let error = format!(
        "[schema:1146]Table '(Schema ID {}).(Table ID {})' doesn't exist",
        job.schema_id, id
    );
    cancel(job, error)
}
fn get(meta: &TransactionMutator<'_>, job: &mut Job, id: i64) -> Result<TableInfo, String> {
    meta.get_table(job.schema_id, id)?
        .ok_or_else(|| missing(job, id))
}
pub fn step(context: &mut dyn JobExecutionContext, job: &mut Job) -> Result<i64, String> {
    let args = astersql_meta_model::group_2::GetCreateMaterializedViewArgs(job)
        .map_err(|e| cancel(job, e))?;
    let table = args
        .TableInfo
        .ok_or_else(|| invalid(job, "invalid job args"))?;
    let mut table: TableInfo =
        serde_json::from_value(serde_json::to_value(table).map_err(|e| cancel(job, e))?)
            .map_err(|e| cancel(job, e))?;
    let info = table
        .MaterializedView
        .as_ref()
        .ok_or_else(|| invalid(job, "invalid job args"))?;
    let ids = info.BaseTableIDs.clone();
    if ids.is_empty() {
        return Err(invalid(job, "invalid job args"));
    }
    let mut seen = std::collections::HashSet::new();
    for id in &ids {
        if *id == 0 {
            return Err(invalid(job, "invalid base table id"));
        }
        if !seen.insert(*id) {
            return Err(invalid(job, "duplicate base table id"));
        }
    }
    if job.state == JobState::Cancelling {
        job.state = JobState::Rollingback;
        return Ok(0);
    }
    if matches!(job.state, JobState::Pausing | JobState::Paused) {
        return Ok(0);
    }
    if job.state != JobState::Rollingback && job.schema_state == SchemaState::WriteReorganization {
        let mut actual = None;
        context.with_transaction(&mut |txn| {
            actual = Some(get(&TransactionMutator::new(txn), job, job.table_id)?);
            Ok(vec![])
        })?;
        let actual = actual.ok_or("materialized view metadata unavailable")?;
        let already_built = job.snapshot_ver != 0;
        match context.build_create_mview_data(job, &actual) {
            Ok((read_ts, count)) => {
                if read_ts == 0 {
                    job.state = JobState::Rollingback;
                    return Err("create materialized view: invalid build read tso".into());
                }
                if !already_built {
                    job.snapshot_ver = read_ts;
                    job.set_row_count(count);
                    return Ok(0);
                }
                context.finish_create_mview_refresh(&job.schema_name, &actual, read_ts)?;
                let mut table = actual;
                table.MaterializedView.as_mut().unwrap().InitBuildState =
                    astersql_meta_model::MViewInitBuildReady;
                let mut finished = Vec::with_capacity(ids.len() + 1);
                let mut version = 0;
                context.with_transaction(&mut |txn| {
                    let mut meta = TransactionMutator::new(txn);
                    meta.update_table(job.schema_id, &mut table)?;
                    for id in &ids {
                        if let Some(base) = meta.get_table(job.schema_id, *id)? {
                            finished.push(std::sync::Arc::new(base));
                        }
                    }
                    version = meta.gen_schema_version()?;
                    meta.set_table_schema_diff(job, version)?;
                    Ok(vec![])
                })?;
                finished.push(std::sync::Arc::new(table));
                job.finish_multiple_table_job(
                    JobState::Done,
                    SchemaState::Public,
                    version,
                    finished,
                );
                return Ok(version);
            }
            Err(error) => {
                job.state = JobState::Rollingback;
                return Err(error);
            }
        }
    }
    if job.state == JobState::Rollingback {
        return rollback(context, job, &table, &ids, &args.MLogTableIDs);
    }
    if job.schema_state != SchemaState::None {
        return Err(format!(
            "invalid create materialized view schema state {}",
            job.schema_state
        ));
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
        for id in &ids {
            let base = get(&meta, job, *id)?;
            if base.View.is_some()
                || base.Sequence.is_some()
                || base.TempTableType != astersql_meta_model::TempTableNone
            {
                return Err(cancel(
                    job,
                    astersql_util_dbterror::ErrWrongObject.GenWithStackByArgs(&[
                        job.schema_name.clone().into(),
                        base.Name.O.clone().into(),
                        "BASE TABLE".into(),
                    ]),
                ));
            }
            if base.GetPartitionInfo().is_some() {
                return Err(cancel(
                    job,
                    astersql_util_dbterror::ErrGeneralUnsupportedDDL.GenWithStackByArgs(&[
                        "CREATE MATERIALIZED VIEW on partition table".into(),
                    ]),
                ));
            }
            if base.State != SchemaState::Public {
                return Err(cancel(
                    job,
                    astersql_util_dbterror::ErrInvalidDDLState
                        .GenWithStackByArgs(&["table".into(), base.State.to_string().into()]),
                ));
            }
            let log_id = base.MaterializedViewBase.as_ref().map_or(0, |b| b.MLogID);
            if log_id == 0 {
                return Err(invalid(job, "base table has no materialized view log"));
            }
            let log = get(&meta, job, log_id)?;
            if !log
                .MaterializedViewLog
                .as_ref()
                .is_some_and(|l| l.BaseTableID == base.ID)
            {
                return Err(invalid(job, "invalid materialized view log metadata"));
            }
            if log.State != SchemaState::Public {
                return Err(cancel(
                    job,
                    astersql_util_dbterror::ErrInvalidDDLState
                        .GenWithStackByArgs(&["table".into(), log.State.to_string().into()]),
                ));
            }
        }
        Ok(vec![])
    })?;
    crate::persistent_create_table::create_table(context, job, &mut table, false)?;
    job.table_id = table.ID;
    let mut affected = Vec::new();
    context.with_transaction(&mut |txn| {
        let mut meta = TransactionMutator::new(txn);
        for id in &ids {
            let mut base = get(&meta, job, *id)?;
            let info = base
                .MaterializedViewBase
                .get_or_insert_with(Default::default);
            if !info.MViewIDs.contains(&table.ID) {
                info.MViewIDs.push(table.ID);
            }
            meta.update_table(job.schema_id, &mut base)
                .map_err(|e| cancel(job, e))?;
            affected.push(base.ID);
        }
        let mut seen = std::collections::HashSet::new();
        for id in &args.MLogTableIDs {
            if *id == 0 {
                return Err(cancel(job, "materialized view log id is invalid"));
            }
            if !seen.insert(*id) {
                continue;
            }
            let mut log = get(&meta, job, *id)?;
            let info = log
                .MaterializedViewLog
                .as_mut()
                .ok_or_else(|| invalid(job, "invalid materialized view log"))?;
            if !ids.contains(&info.BaseTableID) {
                return Err(invalid(
                    job,
                    "materialized view log does not belong to a base table",
                ));
            }
            if info.DependentMViewIDs.contains(&table.ID) {
                continue;
            }
            info.DependentMViewIDs.push(table.ID);
            meta.update_table(job.schema_id, &mut log)
                .map_err(|e| cancel(job, e))?;
            affected.push(log.ID);
        }
        Ok(vec![])
    })?;
    let mut version = 0;
    context.with_transaction(&mut |txn| {
        let mut meta = TransactionMutator::new(txn);
        version = meta.gen_schema_version()?;
        meta.set_create_mlog_schema_diff(job, version, &affected, false)?;
        Ok(vec![])
    })?;
    crate::persistent_actions::async_notify_event(
        context,
        job,
        -1,
        astersql_ddl_notifier::NewCreateTableEvent(Some(Box::new(table.clone()))),
    )?;
    if let Err(e) = context.prewrite_create_mview_refresh(table.ID) {
        job.state = JobState::Rollingback;
        return Err(e);
    }
    // Preserve the complete Go V1/V2 payload for owner recovery, including MLog IDs.
    job.raw_args = serde_json::to_vec(&if job.version == JobVersion::V1 {
        serde_json::json!([table, args.MLogTableIDs])
    } else {
        serde_json::json!({"table_info":table,"mlog_table_ids":args.MLogTableIDs})
    })
    .map_err(|e| e.to_string())?;
    job.schema_state = SchemaState::WriteReorganization;
    job.state = JobState::Running;
    Ok(version)
}

fn rollback(
    context: &mut dyn JobExecutionContext,
    job: &mut Job,
    args: &TableInfo,
    base_ids: &[i64],
    log_ids: &[i64],
) -> Result<i64, String> {
    let mut affected = Vec::new();
    context.with_transaction(&mut |txn| {
        let mut meta = TransactionMutator::new(txn);
        for id in base_ids {
            let Some(mut base) = meta.get_table(job.schema_id, *id)? else {
                continue;
            };
            if let Some(info) = &mut base.MaterializedViewBase {
                info.MViewIDs.retain(|id| *id != job.table_id);
                if info.MLogID == 0 && info.MViewIDs.is_empty() {
                    base.MaterializedViewBase = None;
                }
            }
            meta.update_table(job.schema_id, &mut base)?;
            affected.push(base.ID);
        }
        for id in log_ids {
            let Some(mut log) = meta.get_table(job.schema_id, *id)? else {
                continue;
            };
            if let Some(info) = &mut log.MaterializedViewLog {
                info.DependentMViewIDs.retain(|id| *id != job.table_id);
            }
            meta.update_table(job.schema_id, &mut log)?;
            affected.push(log.ID);
        }
        if meta.get_table(job.schema_id, job.table_id)?.is_some() {
            meta.drop_table_and_auto_ids(job.schema_id, job.table_id)?;
        }
        Ok(vec![])
    })?;
    context.delete_create_mview_refresh(job.table_id)?;
    let mut version = 0;
    context.with_transaction(&mut |txn| {
        let mut meta = TransactionMutator::new(txn);
        version = meta.gen_schema_version()?;
        meta.set_create_mlog_schema_diff(job, version, &affected, true)?;
        Ok(vec![])
    })?;
    job.raw_args = serde_json::to_vec(&if job.version == JobVersion::V1 {
        serde_json::json!([args, log_ids])
    } else {
        serde_json::json!({"table_info":args,"mlog_table_ids":log_ids})
    })
    .map_err(|error| error.to_string())?;
    job.state = JobState::RollbackDone;
    job.schema_state = SchemaState::None;
    Ok(version)
}
