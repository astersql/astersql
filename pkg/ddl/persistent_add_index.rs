// Copyright 2026 AsterSQL.

//! Limited persistent ADD INDEX adapter for the existing distributed backfill.
use crate::job_worker::JobExecutionContext;
use astersql_meta_model::group_2::JobArgsCompat;
use astersql_meta_model::group_3::{Job, JobState};
use astersql_meta_model::{IndexInfo, SchemaState, TableInfo};

fn write(
    context: &mut dyn JobExecutionContext,
    job: &Job,
    table: &mut TableInfo,
) -> Result<i64, String> {
    let mut version = 0;
    context.with_transaction(&mut |txn| {
        let mut meta = astersql_meta::TransactionMutator::new(txn);
        version = meta.gen_schema_version()?;
        meta.set_table_schema_diff(job, version)?;
        meta.update_table(job.schema_id, table)?;
        Ok(Vec::new())
    })?;
    Ok(version)
}

pub fn step(context: &mut dyn JobExecutionContext, job: &mut Job) -> Result<i64, String> {
    let mut args: serde_json::Value =
        serde_json::from_slice(&job.raw_args).map_err(|e| e.to_string())?;
    let mut prepared: IndexInfo =
        serde_json::from_value(args["prepared_index"].clone()).map_err(|e| e.to_string())?;
    let mut table = None;
    context.with_transaction(&mut |txn| {
        table =
            astersql_meta::TransactionMutator::new(txn).get_table(job.schema_id, job.table_id)?;
        Ok(Vec::new())
    })?;
    let mut table = table.ok_or("ADD INDEX table missing")?;
    if matches!(job.state, JobState::Cancelling | JobState::Rollingback) {
        // Cancellation before the first schema step owns no metadata, even if
        // another job has since created the same name. Never remove a public
        // index or use the SQL-side predicted ID as proof of ownership.
        let removed = table
            .Indices
            .iter()
            .find(|index| {
                job.schema_state != SchemaState::None
                    && index.ID == prepared.ID
                    && index.Name.L == prepared.Name.L
                    && index.State != SchemaState::Public
            })
            .cloned();
        let version = if let Some(index) = &removed {
            table.Indices.retain(|candidate| candidate.ID != index.ID);
            write(context, job, &mut table)?
        } else {
            0
        };
        let args = astersql_meta_model::group_2::ModifyIndexArgs {
            IndexArgs: removed
                .into_iter()
                .map(|index| astersql_meta_model::group_2::IndexArg {
                    IndexID: index.ID,
                    ..Default::default()
                })
                .collect(),
            PartitionIDs: table
                .GetPartitionInfo()
                .map(|p| p.Definitions.iter().map(|p| p.ID).collect())
                .unwrap_or_default(),
            ..Default::default()
        };
        job.finish_table_job(
            JobState::RollbackDone,
            SchemaState::None,
            version,
            std::sync::Arc::new(table),
        );
        job.FillFinishedArgs(args);
        return Ok(version);
    }
    if job.schema_state == SchemaState::None {
        if table
            .Indices
            .iter()
            .any(|index| index.Name.L == prepared.Name.L)
        {
            job.state = JobState::Cancelled;
            return Err(format!("duplicate index {}", prepared.Name.O));
        }
        table.MaxIndexID = table
            .MaxIndexID
            .checked_add(1)
            .ok_or("index ID exhausted")?;
        prepared.ID = table.MaxIndexID;
        prepared.State = SchemaState::DeleteOnly;
        prepared.Table = table.Name.clone();
        crate::persistent_actions::initialize_reorg_indexes(
            context,
            job,
            std::slice::from_mut(&mut prepared),
        )?;
        // Persist the ID actually allocated under the DDL transaction. The
        // owner may resume or cancel this job after its in-memory state is lost.
        args["prepared_index"] = serde_json::to_value(&prepared).map_err(|e| e.to_string())?;
        if let Some(first) = args
            .get_mut("index_args")
            .and_then(|args| args.as_array_mut())
            .and_then(|args| args.first_mut())
        {
            first["index_id"] = serde_json::json!(prepared.ID);
        }
        job.raw_args = serde_json::to_vec(&args).map_err(|e| e.to_string())?;
        table.Indices.push(prepared);
        job.schema_state = SchemaState::DeleteOnly;
        return write(context, job, &mut table);
    }
    let position = table
        .Indices
        .iter()
        .position(|index| index.Name.L == prepared.Name.L && index.ID == prepared.ID)
        .ok_or("ADD INDEX prepared metadata missing")?;
    match job.schema_state {
        SchemaState::DeleteOnly => {
            table.Indices[position].State = SchemaState::WriteOnly;
            job.schema_state = SchemaState::WriteOnly;
        }
        SchemaState::WriteOnly => {
            table.Indices[position].State = SchemaState::WriteReorganization;
            job.schema_state = SchemaState::WriteReorganization;
            context.with_transaction(&mut |txn| {
                job.snapshot_ver = txn.StartTS();
                Ok(Vec::new())
            })?;
        }
        SchemaState::WriteReorganization => {
            let meta = job
                .reorg_meta
                .as_ref()
                .ok_or("ADD INDEX reorg metadata missing")?;
            if !meta.IsDistReorg || !meta.UseCloudStorage {
                return Err("limited ADD INDEX adapter requires distributed global sort".into());
            }
            let start = astersql_tablecodec::GenTableRecordPrefix(table.ID).0;
            let end = astersql_kv::Key(start.clone()).PrefixNext().0;
            let request = crate::backfilling::IndexBackfillBatch {
                schema_id: job.schema_id,
                table_id: table.ID,
                index_ids: vec![table.Indices[position].ID],
                task: crate::backfilling::ReorgBackfillTask {
                    physical_table_id: table.ID,
                    start_key: start,
                    end_key: end,
                    ..Default::default()
                },
                batch_size: meta.GetBatchSize().max(1) as usize,
                resource_group: meta.ResourceGroupName.clone(),
                sql_mode: job.sql_mode as i64,
            };
            match table.Indices[position].BackfillState {
                astersql_meta_model::BackfillStateReadyToMerge => {
                    table.Indices[position].BackfillState =
                        astersql_meta_model::BackfillStateMerging;
                    return write(context, job, &mut table);
                }
                astersql_meta_model::BackfillStateMerging => {
                    let rows = job.get_row_count();
                    context.merge_modified_indexes(request, job)?;
                    job.set_row_count(rows);
                }
                _ => {
                    context.ingest_modified_indexes(request, job)?;
                    table.Indices[position].BackfillState =
                        astersql_meta_model::BackfillStateReadyToMerge;
                    return write(context, job, &mut table);
                }
            }
            table.Indices[position].BackfillState = astersql_meta_model::BackfillStateInapplicable;
            table.Indices[position].State = SchemaState::Public;
            job.schema_state = SchemaState::Public;
            let version = write(context, job, &mut table)?;
            let finished = astersql_meta_model::group_2::ModifyIndexArgs {
                IndexArgs: vec![astersql_meta_model::group_2::IndexArg {
                    IndexID: table.Indices[position].ID,
                    ..Default::default()
                }],
                PartitionIDs: table
                    .GetPartitionInfo()
                    .map(|p| p.Definitions.iter().map(|p| p.ID).collect())
                    .unwrap_or_default(),
                ..Default::default()
            };
            job.finish_table_job(
                JobState::Done,
                SchemaState::Public,
                version,
                std::sync::Arc::new(table),
            );
            job.FillFinishedArgs(finished);
            return Ok(version);
        }
        _ => return Err("unexpected ADD INDEX schema state".into()),
    }
    write(context, job, &mut table)
}
