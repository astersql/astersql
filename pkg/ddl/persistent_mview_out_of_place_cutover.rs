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

//! Atomic promotion of a materialized-view shadow table.

use crate::job_worker::JobExecutionContext;
use astersql_meta::TransactionMutator;
use astersql_meta_model::{
    SchemaState, TableInfo,
    group_3::{Job, JobState},
};
use std::collections::HashSet;
use std::sync::Arc;

fn cancel(job: &mut Job, message: impl ToString) -> String {
    job.state = JobState::Cancelled;
    message.to_string()
}

fn invalid(job: &mut Job, detail: &str) -> String {
    cancel(
        job,
        format!("[ddl:8204]refresh materialized view complete OUT OF PLACE cutover: {detail}"),
    )
}

pub(crate) fn replace_materialized_view_id(
    ids: &[i64],
    old_id: i64,
    new_id: i64,
) -> (Vec<i64>, bool) {
    let mut replaced = false;
    let mut seen = HashSet::new();
    let result = ids
        .iter()
        .copied()
        .filter_map(|mut id| {
            if id == old_id {
                id = new_id;
                replaced = true;
            }
            seen.insert(id).then_some(id)
        })
        .collect();
    (result, replaced)
}

pub(crate) fn rewrite_materialized_view_base(
    table: &mut TableInfo,
    old_id: i64,
    new_id: i64,
) -> Result<(), String> {
    let base = table
        .MaterializedViewBase
        .as_mut()
        .ok_or("base table materialized view metadata missing")?;
    let (ids, replaced) = replace_materialized_view_id(&base.MViewIDs, old_id, new_id);
    if !replaced {
        return Err("old materialized view id is missing in base table metadata".into());
    }
    base.MViewIDs = ids;
    Ok(())
}

pub fn step(context: &mut dyn JobExecutionContext, job: &mut Job) -> Result<i64, String> {
    let args =
        astersql_meta_model::group_2::GetRefreshMaterializedViewCompleteOutOfPlaceCutoverArgs(job)
            .map_err(|e| cancel(job, e))?;
    if args.OldMViewID != job.table_id
        || args.OldMViewID == args.ShadowTableID
        || args.BuildReadTSO == 0
    {
        return Err(invalid(job, "invalid table IDs or build read tso"));
    }

    let mut old = None;
    let mut promoted = None;
    let mut affected = Vec::new();
    context.with_transaction(&mut |txn| {
        let meta = TransactionMutator::new(txn);
        old = meta.get_table(job.schema_id, args.OldMViewID)?;
        promoted = meta.get_table(job.schema_id, args.ShadowTableID)?;
        Ok(Vec::new())
    })?;
    let old = old.ok_or_else(|| invalid(job, "old materialized view does not exist"))?;
    let mut promoted = promoted.ok_or_else(|| invalid(job, "shadow table does not exist"))?;
    let mview = old
        .MaterializedView
        .as_ref()
        .ok_or_else(|| invalid(job, "old object is not a materialized view"))?;
    if mview.BaseTableIDs.len() != 1 {
        return Err(invalid(job, "materialized view must have one base table"));
    }
    if args
        .ExpectedOldMViewRevision
        .is_some_and(|revision| revision != old.Revision)
    {
        return Err(invalid(
            job,
            "stale old materialized view revision detected before cutover",
        ));
    }
    if promoted.MaterializedView.is_some()
        || promoted.MaterializedViewLog.is_some()
        || promoted.View.is_some()
        || promoted.Sequence.is_some()
        || promoted
            .MaterializedViewShadow
            .as_ref()
            .map(|s| s.SourceMViewID)
            != Some(args.OldMViewID)
        || promoted.State != SchemaState::Public
    {
        return Err(invalid(job, "invalid shadow table metadata"));
    }

    let base_id = mview.BaseTableIDs[0];
    let mut base = None;
    let mut log = None;
    context.with_transaction(&mut |txn| {
        let meta = TransactionMutator::new(txn);
        base = meta.get_table(job.schema_id, base_id)?;
        if let Some(id) = base
            .as_ref()
            .and_then(|t| t.MaterializedViewBase.as_ref())
            .map(|b| b.MLogID)
            .filter(|id| *id != 0)
        {
            log = meta.get_table(job.schema_id, id)?;
        }
        Ok(Vec::new())
    })?;
    let mut base = base.ok_or_else(|| invalid(job, "base table does not exist"))?;
    rewrite_materialized_view_base(&mut base, args.OldMViewID, args.ShadowTableID)
        .map_err(|e| invalid(job, &e))?;
    let mut keep_log = false;
    if let Some(log_table) = &mut log {
        let info = log_table
            .MaterializedViewLog
            .as_mut()
            .ok_or_else(|| invalid(job, "materialized view log metadata is invalid"))?;
        if info.BaseTableID != base.ID {
            return Err(invalid(job, "materialized view log metadata is invalid"));
        }
        let (ids, replaced) = replace_materialized_view_id(
            &info.DependentMViewIDs,
            args.OldMViewID,
            args.ShadowTableID,
        );
        if replaced {
            info.DependentMViewIDs = ids;
            keep_log = true;
        }
    }
    if !keep_log {
        log = None;
    }

    context
        .migrate_mview_refresh_info(&args)
        .map_err(|e| cancel(job, e))?;
    promoted.Name = old.Name.clone();
    promoted.Comment = old.Comment.clone();
    promoted.MaterializedView = old.MaterializedView.clone();
    promoted.MaterializedViewShadow = None;
    let mut version = 0;
    context.with_transaction(&mut |txn| {
        let mut meta = TransactionMutator::new(txn);
        meta.drop_table_and_auto_ids(job.schema_id, args.OldMViewID)?;
        meta.update_table(job.schema_id, &mut promoted)?;
        meta.update_table(job.schema_id, &mut base)?;
        affected.push(base.ID);
        if let Some(log) = &mut log {
            meta.update_table(job.schema_id, log)?;
            affected.push(log.ID);
        }
        version = meta.gen_schema_version()?;
        meta.set_mview_cutover_schema_diff(
            job,
            version,
            args.OldMViewID,
            args.ShadowTableID,
            &affected,
        )?;
        Ok(Vec::new())
    })?;
    crate::persistent_actions::async_notify_event(
        context,
        job,
        -1,
        astersql_ddl_notifier::NewMViewRefreshOutOfPlaceCutoverEvent(
            Some(Box::new(promoted.clone())),
            Some(Box::new(old)),
        ),
    )?;
    job.finish_table_job(
        JobState::Done,
        SchemaState::Public,
        version,
        Arc::new(promoted),
    );
    Ok(version)
}
